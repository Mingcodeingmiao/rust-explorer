// Explicit system-API probe for an owned fixture, without association writes.
#include "ComSupport.h"
#include <shlobj.h>
#include <exdisp.h>
#include <propvarutil.h>
#include <cstdio>

int wmain(int argc, wchar_t** argv) {
    if (argc != 3 && argc != 4) return 2;
    auto hr = OleInitialize(nullptr);
    if (FAILED(hr)) return 3;
    IShellWindows* windows = nullptr;
    PIDLIST_ABSOLUTE folder = nullptr, item = nullptr;
    hr=SHParseDisplayName(argv[1], nullptr, &folder, 0, nullptr);
    if (FAILED(hr)) return 4;
    hr=SHParseDisplayName(argv[2], nullptr, &item, 0, nullptr);
    if (FAILED(hr)) return 5;
    if (SUCCEEDED(CoCreateInstance(CLSID_ShellWindows,nullptr,CLSCTX_ALL,IID_IShellWindows,reinterpret_cast<void**>(&windows)))) {
        long count=0; windows->get_Count(&count); std::printf("COUNT %ld\n",count);
        for (long index=0; index<count; ++index) {
            LocalVariant key; key.vt=VT_I4; key.lVal=index;
            IDispatch* object=nullptr;
            if (SUCCEEDED(windows->Item(key,&object)) && object) {
                IWebBrowserApp* browser=nullptr;
                if (SUCCEEDED(object->QueryInterface(IID_IWebBrowserApp,reinterpret_cast<void**>(&browser)))) {
                    SHANDLE_PTR window=0; browser->get_HWND(&window);
                    std::printf("ITEM %ld HWND=%lld\n",index,static_cast<long long>(window)); browser->Release();
                }
                object->Release();
            }
        }
        for (int type : {SWC_EXPLORER,SWC_BROWSER}) {
            LocalVariant path, empty;
            InitVariantFromBuffer(folder,ILGetSize(folder),&path);
            long hwnd=0;
            IDispatch* dispatch=nullptr;
            hr=windows->FindWindowSW(&path,&empty,type,&hwnd,SWFO_NEEDDISPATCH | SWFO_INCLUDEPENDING,&dispatch);
            std::printf("FIND class=%d hr=%08lx hwnd=%ld dispatch=%d\n",type,static_cast<unsigned long>(hr),hwnd,dispatch != nullptr);
            if (dispatch) dispatch->Release();
        }
        windows->Release();
    }
    if (argc == 3) {
        PCUITEMID_CHILD child=ILFindLastID(item);
        hr=SHOpenFolderAndSelectItems(folder,1,&child,0);
        std::printf("SHOpenFolderAndSelectItems=%08lx\n",static_cast<unsigned long>(hr));
    }
    CoTaskMemFree(item); CoTaskMemFree(folder); OleUninitialize();
    return FAILED(hr) ? 6 : 0;
}
