// Rust Explorer experiment. Does not install or change folder associations.
#include "OpenInFolder.h"
#include <cstdio>

static LRESULT CALLBACK WindowProc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp) {
    auto bridge = reinterpret_cast<OpenInFolder*>(GetWindowLongPtrW(hwnd, GWLP_USERDATA));
    if (msg == WM_NCCREATE) {
        bridge = static_cast<OpenInFolder*>(reinterpret_cast<CREATESTRUCTW*>(lp)->lpCreateParams);
        if (!bridge) return FALSE;
        bridge->SetWindow(hwnd);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(bridge));
    }
    if (bridge) return bridge->WindowProcedure(hwnd, msg, wp, lp);
    return DefWindowProcW(hwnd, msg, wp, lp);
}

static bool Launch(const std::wstring& exe, const std::wstring& target, bool select) {
    // Windows filenames cannot contain a quote. Reject it instead of accepting arguments.
    if (exe.find(L'"') != std::wstring::npos || target.find(L'"') != std::wstring::npos) return false;
    auto quote = [](const std::wstring& value) {
        auto end=value.find_last_not_of(L'\\');
        auto trailing=end == std::wstring::npos ? value.size() : value.size()-end-1;
        return L"\"" + value + std::wstring(trailing,L'\\') + L"\"";
    };
    std::wstring command = quote(exe) + (select ? L" --select " : L" --open-folder ") + quote(target);
    STARTUPINFOW startup{}; startup.cb = sizeof(startup);
    PROCESS_INFORMATION process{};
    if (!CreateProcessW(exe.c_str(), command.data(), nullptr, nullptr, FALSE,
                        CREATE_NO_WINDOW, nullptr, nullptr, &startup, &process)) return false;
    CloseHandle(process.hThread); CloseHandle(process.hProcess);
    return true;
}

int wmain(int argc, wchar_t** argv) {
    if (argc != 3) { std::fprintf(stderr, "Usage: shell-bridge.exe FOLDER RUST-EXPLORER-EXE\n"); return 2; }
    HRESULT hr = OleInitialize(nullptr);
    if (FAILED(hr)) return 3;
    auto bridge = new OpenInFolder(argv[1]);
    auto module = GetModuleHandleW(nullptr);
    WNDCLASSW wc{}; wc.lpfnWndProc=WindowProc; wc.hInstance=module; wc.lpszClassName=L"RustExplorer.Experimental.ShellBridge";
    if (!RegisterClassW(&wc)) { bridge->Release(); OleUninitialize(); return 4; }
    HWND hwnd = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, wc.lpszClassName,
        L"RE experimental shell bridge", 0, 0, 0, 0, 0, nullptr, nullptr, module, bridge);
    int result = 0;
    if (!hwnd || !bridge->Ready()) { std::fprintf(stderr,"Shell registration failed\n"); result=5; }
    else {
        std::printf("READY\n"); std::fflush(stdout);
        auto start = GetTickCount64();
        bool folderSent = false;
        while (bridge->GetResult().empty() && GetTickCount64()-start < 10000) {
            MSG message{};
            while (PeekMessageW(&message, nullptr, 0, 0, PM_REMOVE)) {
                if (message.message != WM_QUIT) { TranslateMessage(&message); DispatchMessageW(&message); }
            }
            if (!bridge->GetResult().empty()) break;
            if (!folderSent && GetTickCount64()-start >= 500) {
                folderSent=true;
                if (!Launch(argv[2],argv[1],false)) { result=6; break; }
                std::printf("DIRECTORY\n"); std::fflush(stdout);
            }
            MsgWaitForMultipleObjects(0,nullptr,FALSE,20,QS_ALLINPUT);
        }
        auto selected=bridge->GetResult();
        if (!selected.empty()) {
            if (!Launch(argv[2],selected,true)) result=7;
            else std::printf("SELECTED\n");
        } else if (!result) std::printf("NO_SELECTION\n");
    }
    bridge->RevokeShellWindow();
    if (hwnd && IsWindow(hwnd)) DestroyWindow(hwnd);
    bridge->Release();
    UnregisterClassW(wc.lpszClassName,module);
    OleUninitialize();
    return result;
}
