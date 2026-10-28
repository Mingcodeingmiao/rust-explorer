// OLE source and target are deliberately separate from context-menu dispatch.
#include "OpenInFolder.h"
#include <new>
#include <cstdio>
#include <commctrl.h>

static void drag_trace(const char* stage) {
    wchar_t path[32768]{};
    if (GetEnvironmentVariableW(L"RUST_EXPLORER_DRAG_TRACE", path, 32768)) {
        if (auto file = _wfopen(path, L"a")) { fprintf(file, "%s\n", stage); fclose(file); }
    }
}

using ResolveDrop = int (*)(int, float, float, wchar_t*, unsigned, void*);
// Optimized Shell moves can return NONE even though the target already moved
// the files. Use the logical effect for UI refresh only; NEVER rewrite the
// raw effect returned to an external source (it could delete the files twice).
static DWORD logical_effect(IDataObject* data, DWORD fallback) {
    FORMATETC format{static_cast<CLIPFORMAT>(RegisterClipboardFormatW(L"Logical Performed DropEffect")),
                     nullptr, DVASPECT_CONTENT, -1, TYMED_HGLOBAL};
    STGMEDIUM medium{};
    if (FAILED(data->GetData(&format, &medium))) return fallback;
    DWORD effect = fallback;
    if (medium.tymed == TYMED_HGLOBAL && GlobalSize(medium.hGlobal) >= sizeof(DWORD)) {
        if (auto value = static_cast<DWORD*>(GlobalLock(medium.hGlobal))) {
            effect = *value & (DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK);
            GlobalUnlock(medium.hGlobal);
        }
    }
    ReleaseStgMedium(&medium);
    return effect;
}
static constexpr UINT START_FILE_DRAG = WM_APP + 0x637;
static constexpr UINT_PTR DRAG_SUBCLASS = 0x52454444;
extern "C" HRESULT re_drag_files(HWND, const wchar_t* const*, unsigned, DWORD*) noexcept;

// Finish reliably even if the cursor crosses into another application's window.
class FileDropSource final : public IDropSource {
    LONG refs_ = 1;
    bool observed_ = false;
public:
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** out) override {
        if (!out) return E_POINTER;
        *out = nullptr;
        if (iid == IID_IUnknown || iid == IID_IDropSource) {
            *out = static_cast<IDropSource*>(this); AddRef(); return S_OK;
        }
        return E_NOINTERFACE;
    }
    ULONG STDMETHODCALLTYPE AddRef() override { return InterlockedIncrement(&refs_); }
    ULONG STDMETHODCALLTYPE Release() override {
        const auto count = InterlockedDecrement(&refs_); if (!count) delete this; return count;
    }
    HRESULT STDMETHODCALLTYPE QueryContinueDrag(BOOL escape, DWORD) override {
        if (!observed_) { observed_ = true; drag_trace("query continue drag"); }
        if (escape || (GetAsyncKeyState(VK_ESCAPE) & 0x8000)) return DRAGDROP_S_CANCEL;
        return (GetAsyncKeyState(VK_LBUTTON) & 0x8000) ? S_OK : DRAGDROP_S_DROP;
    }
    HRESULT STDMETHODCALLTYPE GiveFeedback(DWORD) override { return DRAGDROP_S_USEDEFAULTCURSORS; }
};

class FileDropTarget final : public IDropTarget {
    LONG refs_ = 1;
    HWND owner_;
    ResolveDrop resolve_;
    void* context_;
    IDataObject* data_ = nullptr;
    IDropTarget* target_ = nullptr;
    IDropTargetHelper* helper_ = nullptr;
    std::wstring path_;
    std::vector<std::wstring> pending_;
    bool source_active_ = false;

    void leave() {
        if (target_) { target_->DragLeave(); target_->Release(); target_ = nullptr; }
        path_.clear();
    }
    void reset() {
        leave();
        if (data_) { data_->Release(); data_ = nullptr; }
    }
    HRESULT locate(DWORD keys, POINTL point, DWORD* effect) {
        try {
        POINT client{point.x, point.y};
        ScreenToClient(owner_, &client);
        const float scale = GetDpiForWindow(owner_) / 96.f;
        wchar_t destination[32768]{};
        const bool valid = resolve_(0, client.x / scale, client.y / scale,
                                   destination, 32768, context_) != 0;
        std::wstring next = valid ? destination : L"";
        if (next != path_) {
            leave();
            if (!next.empty()) {
                ComPtr<IShellItem> item;
                HRESULT hr = SHCreateItemFromParsingName(next.c_str(), nullptr, IID_PPV_ARGS(item.put()));
                if (SUCCEEDED(hr)) hr = item->BindToHandler(nullptr, BHID_SFUIObject, IID_PPV_ARGS(&target_));
                if (FAILED(hr)) { *effect = DROPEFFECT_NONE; return S_OK; }
                path_ = next;
                return target_->DragEnter(data_, keys, point, effect);
            }
        }
        if (!target_) { *effect = DROPEFFECT_NONE; return S_OK; }
        return target_->DragOver(keys, point, effect);
        } catch (...) {
            leave();
            *effect = DROPEFFECT_NONE;
            return E_OUTOFMEMORY;
        }
    }
public:
    HRESULT queue(const wchar_t* const* paths, unsigned count) {
        if (source_active_ || !pending_.empty()) return E_PENDING;
        if (!paths || !count) return E_INVALIDARG;
        try {
            for (unsigned i = 0; i < count; ++i) pending_.emplace_back(paths[i]);
        } catch (...) { pending_.clear(); return E_OUTOFMEMORY; }
        if (!PostMessageW(owner_, START_FILE_DRAG, 0, 0)) {
            pending_.clear(); return HRESULT_FROM_WIN32(GetLastError());
        }
        return S_OK;
    }
    void run_pending() {
        if (pending_.empty() || source_active_) return;
        auto paths = std::move(pending_);
        source_active_ = true;
        HRESULT hr = E_OUTOFMEMORY;
        DWORD effect = 0;
        try {
            std::vector<const wchar_t*> pointers;
            for (auto& path : paths) pointers.push_back(path.c_str());
            hr = re_drag_files(owner_, pointers.data(), static_cast<unsigned>(pointers.size()), &effect);
        } catch (...) {}
        source_active_ = false;
        wchar_t error[96]{};
        swprintf(error, 96, L"Windows drag/drop failed: 0x%08X", static_cast<unsigned>(hr));
        resolve_(FAILED(hr) ? 4 : 3, static_cast<float>(effect), 0, error, 96, context_);
    }
    FileDropTarget(HWND owner, ResolveDrop resolve, void* context)
        : owner_(owner), resolve_(resolve), context_(context) {
        CoCreateInstance(CLSID_DragDropHelper, nullptr, CLSCTX_INPROC_SERVER,
                         IID_PPV_ARGS(&helper_));
    }
    ~FileDropTarget() {
        reset();
        if (helper_) helper_->Release();
        resolve_(2, 0, 0, nullptr, 0, context_);
    }
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** out) override {
        if (!out) return E_POINTER;
        *out = nullptr;
        if (iid == IID_IUnknown || iid == IID_IDropTarget) {
            *out = static_cast<IDropTarget*>(this); AddRef(); return S_OK;
        }
        return E_NOINTERFACE;
    }
    ULONG STDMETHODCALLTYPE AddRef() override { return InterlockedIncrement(&refs_); }
    ULONG STDMETHODCALLTYPE Release() override {
        auto count = InterlockedDecrement(&refs_); if (!count) delete this; return count;
    }
    HRESULT STDMETHODCALLTYPE DragEnter(IDataObject* data, DWORD keys, POINTL point, DWORD* effect) override {
        reset();
        if (!data || !effect) return E_POINTER;
        data_ = data; data_->AddRef();
        const auto hr = locate(keys, point, effect);
        POINT p{point.x, point.y};
        if (helper_) helper_->DragEnter(owner_, data, &p, *effect);
        return hr;
    }
    HRESULT STDMETHODCALLTYPE DragOver(DWORD keys, POINTL point, DWORD* effect) override {
        if (!effect) return E_POINTER;
        const auto hr = locate(keys, point, effect);
        POINT p{point.x, point.y};
        if (helper_) helper_->DragOver(&p, *effect);
        return hr;
    }
    HRESULT STDMETHODCALLTYPE DragLeave() override {
        if (helper_) helper_->DragLeave();
        reset();
        return S_OK;
    }
    HRESULT STDMETHODCALLTYPE Drop(IDataObject* data, DWORD keys, POINTL point, DWORD* effect) override {
        if (!effect) return E_POINTER;
        auto hr = locate(keys, point, effect);
        if (SUCCEEDED(hr) && target_) hr = target_->Drop(data, keys, point, effect);
        else *effect = DROPEFFECT_NONE;
        // Drop terminates the delegate's drag lifecycle; do not call DragLeave again.
        if (target_) { target_->Release(); target_ = nullptr; }
        POINT p{point.x, point.y};
        const auto logical = SUCCEEDED(hr) ? logical_effect(data, *effect) : DROPEFFECT_NONE;
        if (helper_) helper_->Drop(data, &p, logical);
        reset();
        if (SUCCEEDED(hr) && logical != DROPEFFECT_NONE)
            resolve_(1, 0, 0, nullptr, 0, context_);
        return hr;
    }
};

static LRESULT CALLBACK DragSubclass(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp,
                                     UINT_PTR, DWORD_PTR context) {
    auto target = reinterpret_cast<FileDropTarget*>(context);
    target->AddRef();
    LRESULT result;
    if (msg == START_FILE_DRAG) {
        target->run_pending();
        result = 0;
    } else {
        if (msg == WM_NCDESTROY) RemoveWindowSubclass(hwnd, DragSubclass, DRAG_SUBCLASS);
        result = DefSubclassProc(hwnd, msg, wp, lp);
    }
    target->Release();
    return result;
}

extern "C" HRESULT re_register_file_drop(HWND owner, ResolveDrop callback, void* context,
                                         void** registration) noexcept {
    if (!owner || !callback || !registration) {
        if (callback && context) callback(2, 0, 0, nullptr, 0, context);
        return E_INVALIDARG;
    }
    *registration = nullptr;
    auto target = new (std::nothrow) FileDropTarget(owner, callback, context);
    if (!target) { callback(2, 0, 0, nullptr, 0, context); return E_OUTOFMEMORY; }
    // GPUI registers a CF_HDROP-only target. Replace it on the same UI STA.
    HRESULT hr = RevokeDragDrop(owner);
    if (hr != DRAGDROP_E_NOTREGISTERED && FAILED(hr)) { target->Release(); return hr; }
    hr = RegisterDragDrop(owner, target);
    if (FAILED(hr)) { target->Release(); return hr; }
    if (!SetWindowSubclass(owner, DragSubclass, DRAG_SUBCLASS, reinterpret_cast<DWORD_PTR>(target))) {
        RevokeDragDrop(owner); target->Release(); return E_FAIL;
    }
    *registration = target;
    return S_OK;
}
extern "C" void re_unregister_file_drop(HWND owner, void* registration) noexcept {
    if (IsWindow(owner)) RemoveWindowSubclass(owner, DragSubclass, DRAG_SUBCLASS);
    if (IsWindow(owner)) RevokeDragDrop(owner);
    if (registration) static_cast<FileDropTarget*>(registration)->Release();
}
extern "C" HRESULT re_queue_file_drag(void* registration, const wchar_t* const* paths, unsigned count) noexcept {
    if (!registration) return E_INVALIDARG;
    return static_cast<FileDropTarget*>(registration)->queue(paths, count);
}

// Integration-test entry point: no injected input and no user folders. Uses the
// same Shell IDataObject and FileDropTarget as real inter-application drops.
extern "C" HRESULT re_test_file_drop(const wchar_t* source, const wchar_t* destination,
                                     DWORD keys, DWORD* effect) noexcept {
    if (!source || !destination || !effect) return E_INVALIDARG;
    const auto initialized = OleInitialize(nullptr);
    if (FAILED(initialized)) return initialized;
    struct OleRef { ~OleRef() { OleUninitialize(); } } ole;
    PIDLIST_ABSOLUTE pidl = nullptr;
    HWND hwnd = nullptr;
    HRESULT hr = E_FAIL;
    try {
        hr = SHParseDisplayName(source, nullptr, &pidl, 0, nullptr);
        if (SUCCEEDED(hr)) {
            ComPtr<IShellItemArray> items;
            PCIDLIST_ABSOLUTE ids[] = {pidl};
            hr = SHCreateShellItemArrayFromIDLists(1, ids, items.put());
            if (SUCCEEDED(hr)) {
                ComPtr<IDataObject> data;
                hr = items->BindToHandler(nullptr, BHID_DataObject, IID_PPV_ARGS(data.put()));
                if (SUCCEEDED(hr)) {
                    // Explorer and many third-party apps consume CF_HDROP.
                    FORMATETC format{CF_HDROP, nullptr, DVASPECT_CONTENT, -1, TYMED_HGLOBAL};
                    STGMEDIUM medium{};
                    hr = data->GetData(&format, &medium);
                    if (SUCCEEDED(hr)) {
                        const auto count = DragQueryFileW(reinterpret_cast<HDROP>(medium.hGlobal), 0xFFFFFFFF, nullptr, 0);
                        ReleaseStgMedium(&medium);
                        if (count != 1) hr = E_FAIL;
                    }
                    if (SUCCEEDED(hr)) {
                        hwnd = CreateWindowExW(0, L"STATIC", L"Rust Explorer OLE test",
                            0, 0, 0, 1, 1, HWND_MESSAGE, nullptr, GetModuleHandleW(nullptr), nullptr);
                        if (!hwnd) hr = HRESULT_FROM_WIN32(GetLastError());
                        else {
                            auto callback = [](int kind, float, float, wchar_t* buffer, unsigned capacity, void* context) -> int {
                                if (kind != 0) return 1;
                                auto path = static_cast<const wchar_t*>(context);
                                if (wcslen(path) + 1 > capacity) return 0;
                                wcscpy(buffer, path); return 1;
                            };
                            ComPtr<IDropTarget> target;
                            *target.put() = new FileDropTarget(hwnd, callback, const_cast<wchar_t*>(destination));
                            *effect = DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK;
                            POINTL point{};
                            hr = target->DragEnter(data.get(), keys | MK_LBUTTON, point, effect);
                            if (SUCCEEDED(hr) && *effect) {
                                *effect = DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK;
                                hr = target->Drop(data.get(), keys, point, effect);
                                if (SUCCEEDED(hr)) *effect = logical_effect(data.get(), *effect);
                            } else { target->DragLeave(); }
                        }
                    }
                }
            }
        }
    } catch (...) { hr = E_OUTOFMEMORY; }
    if (hwnd) DestroyWindow(hwnd);
    CoTaskMemFree(pidl);
    return hr;
}

extern "C" HRESULT re_drag_files(HWND owner, const wchar_t* const* paths, unsigned count,
                                 DWORD* effect) noexcept {
    if (!paths || !count || !effect) return E_INVALIDARG;
    const auto init = OleInitialize(nullptr);
    drag_trace("ole initialized");
    if (FAILED(init)) return init;
    HRESULT hr = E_FAIL;
    // Every COM interface and PIDL stays on the owning window STA until the drag ends.
    {
        std::vector<PIDLIST_ABSOLUTE> ids;
        try {
            ids.resize(count, nullptr);
            hr = S_OK;
            for (unsigned i = 0; i < count && SUCCEEDED(hr); ++i)
                hr = SHParseDisplayName(paths[i], nullptr, &ids[i], 0, nullptr);
            drag_trace("pidls parsed");
            if (SUCCEEDED(hr)) {
                ComPtr<IShellItemArray> items;
                hr = SHCreateShellItemArrayFromIDLists(count,
                    const_cast<PCIDLIST_ABSOLUTE*>(ids.data()), items.put());
                if (SUCCEEDED(hr)) {
                    ComPtr<IDataObject> data;
                    hr = items->BindToHandler(nullptr, BHID_DataObject, IID_PPV_ARGS(data.put()));
                    drag_trace("data object created");
                    if (SUCCEEDED(hr)) {
                        ComPtr<IDropSource> source;
                        *source.put() = new FileDropSource();
                        drag_trace("enter drag loop");
                        // Called from our window subclass after the GPUI input
                        // callback has returned, so OLE may safely pump messages.
                        hr = SHDoDragDrop(owner, data.get(), source.get(),
                            DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK, effect);
                        if (hr == DRAGDROP_S_DROP) *effect = logical_effect(data.get(), *effect);
                        drag_trace("exit drag loop");
                    }
                }
            }
        } catch (...) { hr = E_OUTOFMEMORY; }
        for (auto id : ids) CoTaskMemFree(id);
    }
    OleUninitialize();
    return hr;
}
