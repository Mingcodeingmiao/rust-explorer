// Rust Explorer's bounded Shell callback host. No association writes.
#include "OpenInFolder.h"
#include <algorithm>

using Delivery = int (*)(const wchar_t*, int, void*);
static constexpr auto CLASS_NAME = L"RustExplorer.ShellBridge.v1";

static LRESULT CALLBACK BridgeProc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp) {
    auto bridge = reinterpret_cast<OpenInFolder*>(GetWindowLongPtrW(hwnd, GWLP_USERDATA));
    if (msg == WM_NCCREATE) {
        bridge = static_cast<OpenInFolder*>(reinterpret_cast<CREATESTRUCTW*>(lp)->lpCreateParams);
        if (!bridge) return FALSE;
        bridge->SetWindow(hwnd);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(bridge));
    }
    return bridge ? bridge->WindowProcedure(hwnd,msg,wp,lp) : DefWindowProcW(hwnd,msg,wp,lp);
}

struct Session {
    bool ole = false;
    ATOM registered = 0;
    HINSTANCE module = GetModuleHandleW(nullptr);
    HWND hwnd = nullptr;
    OpenInFolder* bridge = nullptr;
    ~Session() {
        if (bridge) bridge->RevokeShellWindow();
        if (hwnd && IsWindow(hwnd)) DestroyWindow(hwnd);
        if (bridge) bridge->Release();
        if (registered) UnregisterClassW(CLASS_NAME,module);
        if (ole) OleUninitialize();
    }
};

// kind: 0 registered, 1 directory fallback, 2 selected item. Never unwind across FFI.
extern "C" int re_shell_bridge(const wchar_t* folder, Delivery delivery, void* context,
                              unsigned int initial_ms, unsigned int lifetime_ms) noexcept {
    try {
        if (!folder || !*folder || !delivery || lifetime_ms < initial_ms || lifetime_ms > 10000) return 2;
        Session session;
        if (FAILED(OleInitialize(nullptr))) return 3;
        session.ole = true;
        session.bridge = new OpenInFolder(folder);
        WNDCLASSW wc{}; wc.lpfnWndProc=BridgeProc; wc.hInstance=session.module; wc.lpszClassName=CLASS_NAME;
        session.registered=RegisterClassW(&wc);
        if (!session.registered) return 4;
        session.hwnd=CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,CLASS_NAME,
            L"Rust Explorer Shell bridge",0,0,0,0,0,nullptr,nullptr,session.module,session.bridge);
        if (!session.hwnd || !session.bridge->Ready()) return 5;
        if (!delivery(folder,0,context)) return 6;
        auto start=GetTickCount64();
        bool sentFolder=false;
        while (GetTickCount64()-start < lifetime_ms) {
            MSG message{};
            while (PeekMessageW(&message,nullptr,0,0,PM_REMOVE)) {
                if (message.message != WM_QUIT) { TranslateMessage(&message); DispatchMessageW(&message); }
            }
            auto selected=session.bridge->GetResult();
            if (!selected.empty()) return delivery(selected.c_str(),2,context) ? 0 : 7;
            if (!sentFolder && GetTickCount64()-start >= initial_ms) {
                sentFolder=true;
                if (!delivery(folder,1,context)) return 6;
            }
            MsgWaitForMultipleObjects(0,nullptr,FALSE,20,QS_ALLINPUT);
        }
        return 0;
    } catch (...) { return 8; }
}
