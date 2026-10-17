# 字体、原生菜单与共享入口

实现依据与兼容边界，核对日期：2026-10-17。

## 字体样张

`.ttf`、`.otf`、`.ttc` 在隔离的预览/缩略图 helper 中读取 OpenType `name` 表，取真实字体家族名称，再以 GDI `AddFontResourceExW(FR_PRIVATE)` 临时加载。退出函数时移除私有字体；不写字体注册表、不安装至系统字体目录。TTC 目前取首个 face；可变字体不提供轴编辑。32 MB 以上字体不自动生成样张。

小缩略图显示 Aa 和数字，大预览显示字母、数字和英文例句。按 OS/2 表的字重和斜体标记选取样式，字体选择结果与该 face 的本地化家族名称比对，防止把替代字体误当作原字体。真实 GDI 回归覆盖 Segoe UI 常规、粗体、斜体样张存在差异。`.fon` 只请求系统真正的缩略图提供程序；不可用则保留图标，不猜测 bitmap 字体的私有格式。

API：`preview::Kind::Font`、`native_assets::PreviewKind::Font`；`native_preview::font_sample(path, pixels)` 仅供后台 helper 使用。PNG 缓存沿用文件长度、修改时间和请求分辨率的失效规则。预览调用超时为 1.8 秒，缩略图继续遵循现有可见区域队列和取消机制。

来源：[OpenType name 表](https://learn.microsoft.com/en-us/typography/opentype/spec/name)、[OpenType OS/2 表](https://learn.microsoft.com/en-us/typography/opentype/spec/os2)、[OpenType 字体集合](https://learn.microsoft.com/en-us/typography/opentype/spec/otff#font-collections)、[AddFontResourceEx](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/nf-wingdi-addfontresourceexa)。

## 传统原生菜单

0.6.1 中 `shell::Request::ThemedMenu` 对文件、文件夹和驱动器恢复真正的 Shell `IContextMenu`，由对象的 Shell 父文件夹 `GetUIObjectOf` 生成完整菜单。驱动器根路径自然取得驱动器专用命令，不再拿普通文件夹的精简菜单替代。背景的查看、排序、分组、刷新仍由本程序构建 Win32 popup，选择后返回 `command:<menu_command id>` 给 GPUI 执行，避免交给不存在的 Explorer 视图宿主。

当前用户要求恢复完整系统菜单，覆盖早先放弃扩展兼容的限制：对象菜单会包含系统在当前机器上注册并返回的扩展项目；本程序不再自己枚举一个固定精简集合。外部扩展的显示行为由扩展负责，不承诺每个第三方扩展的私有绘制都兼容暗色。旧 `Request::Menu` 仍可用。

此电脑使用真实 Computer namespace `{20D04FE0-3AEA-1069-A2D8-08002B30309D}` 对象菜单，保留管理、映射/断开网络驱动器、固定开始、删除、属性等系统可用项，额外加入应用自己的树展开、标签页、新窗口命令。添加网络位置遵循本机 Microsoft CommandStore 的 System32 `rundll32 shwebsvc.dll,AddNetPlaceRunDll` 向导。是否显示 BitLocker、弹出、断开取决于设备类型、状态、Windows 功能与策略；本机只读盘点只有固定磁盘，未假造 USB/SMB/BitLocker 状态或实际执行这些操作。

此电脑重命名使用独立原生输入对话框，确认后调用 Desktop `IShellFolder::SetNameOf`，只改 Shell 显示名称；取消不修改。成功回传显示名并更新应用内存缓存，启动和定期查询也在后台 helper 中完成。侧栏、标签、面包屑与地址显示使用缓存，UI 线程不查询 Shell。它不会修改 Windows 主机名，也不会要求系统重启。

验证接口 `InspectMenu` 仅返回标签、canonical verb、ID 和子菜单，不显示弹出菜单、不 Invoke。0.6.1 本机盘点：普通文件和文件夹各四十余项，系统盘包含格式化/重命名/属性，此电脑包含管理和映射/断开网络驱动器；没有执行格式化、加密、弹出、断开或权限更改。依据：[GetUIObjectOf](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-ishellfolder-getuiobjectof)、[QueryContextMenu](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-icontextmenu-querycontextmenu)、[SetNameOf](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-ishellfolder-setnameof)。

暗色请求只影响隔离 helper。Windows 10 build 18362 以上尝试 uxtheme 的 133/135/136 号可选入口及窗口主题；这三个 ordinal 不是公开稳定契约。符号缺失或版本不满足时保留系统默认菜单，不强行套用不兼容地址。此实现没有承诺所有 Windows 版本、辅助功能高对比度设置和经典主题都能完整复刻 Explorer。微软的[Win32 暗色指导](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/ui/apply-windows-themes)也区分标题栏支持和应用控件自行适配；该文档不保证这些私有菜单入口。

## 授予、删除访问权限入口

文件夹项目和文件夹空白区域右键增加“授予访问权限”；空白区域操作作用于当前目录，虚拟主页禁用。子菜单先显示“删除访问权限（系统设置）…”，再显示“特定用户…”。后者请求系统 `ntshrui.dll!ShowShareFolderUI(HWND, LPCWSTR)`；前者也打开 Windows 共享窗口，由用户在系统界面中选择停止共享或调整访问权限。本程序不直接调用 CreateShare、DeleteShare，不写 ACL，不执行未知 unshare verb。系统 UI 的具体布局取决于 Windows 版本和共享配置；这不是对“指定用户/删除访问”两个 Explorer 向导布局的完全重建。

本机只读证据：`C:\Windows\System32\ntshrui.dll` 导出表存在 ShowShareFolderUI（ordinal 16，RVA 0x2cac0），不存在 SharingWizardW；Microsoft Sharing CLSID `{f81e9010-6ea4-11ce-a7ff-00aa003ca9f6}` 注册的 InProcServer32 指向该 DLL。使用本机 objdump 导出表和注册表核对，没有声称完成该 DLL 的控制流反编译。入口从 Windows System32 加载，不执行用户导入的图标资源 DLL。该入口不可用时调用系统 Sharing 属性页。

官方依据：[ShowShareFolderUI](https://learn.microsoft.com/en-us/windows/win32/shell/showsharefolderui)、[SHObjectProperties](https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/nf-shlobj_core-shobjectproperties)。[ISharingConfigurationManager](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nn-shobjidl_core-isharingconfigurationmanager) 针对 Users/Public 和打印机，不能据此实现任意文件夹的“特定用户”。

源码：`src/native_preview.rs`、`src/native_assets.rs`、`src/native_menu.rs`、`src/shell.rs`、`src/ui_extras.rs`。
