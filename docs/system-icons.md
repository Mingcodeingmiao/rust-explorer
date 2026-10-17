# Windows 系统图标来源与回退

0.3 更新：现代工具栏优先直接引用安装在 Windows FileExp 包内的 SVG，见 `fluent-icons.md`。以下 DLL/MUN 映射保留为历史调查及文件/文件夹图标后端；旧的系统图标接触表已移出工程，工程和发布包不包含系统图像副本。

`src/system_icons.rs` 提供阻塞的 `file_icon(&Path, bool, u32)`、
`ui_icon(&str, u32)`，返回 `IconBitmap { width, height, rgba }`。
RGBA 是从上往下、非预乘 alpha；调用方负责缓存与放进限时后台进程。
支持 8–256 像素，过大请求会直接拒绝。渲染路径没有浏览器或 WebView。

## 关联与系统资源

普通文件通过 `SHGetFileInfoW(SHGFI_ICON | SHGFI_USEFILEATTRIBUTES)`
按扩展名取当前 Windows 文件关联图标，不读取文件正文、不提取视频帧。
若该系统图标索引等于 `SIID_DOCNOASSOC`（系统未关联白纸图标），
视频、图片与音频扩展名分别尝试 `SIID_VIDEOFILES`、`SIID_IMAGEFILES`、
`SIID_AUDIOFILES`；这些兜底图标仍来自 Windows。
绝对路径的 EXE、LNK、ICO、CUR 可以读取每项自己的 Shell 图标；调用方
仅传 `file.exe` 这样的缓存键时，仍按文件关联查找，不把假路径当真实文件。
文件夹使用 `SHGetStockIconInfo(SIID_FOLDER)`，磁盘使用驱动器类别的
stock icon。已知目录先用 Windows Known Folder API 识别实际位置，
再用对应的已验证系统资源。目录识别使用 `KF_FLAG_DONT_VERIFY`，支持重定向。

界面资源通过 `GetSystemDirectoryW` 定位系统目录，然后依次尝试系统 DLL
与 `Windows/SystemResources/<DLL>.mun`。`LoadLibraryExW` 仅使用
`LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE`，不运行模块代码。
`LoadImageW` 按资源 ID 和实际 DPI 像素尺寸读取。系统资源 ID 不是公开稳定的
Windows API 合约，加载失败会返回错误，由 GPUI 原有 Fluent/自绘图标回退。

| 界面语义 | 来源 |
| --- | --- |
| 向上 | shell32.dll / shell32.dll.mun #16817 |
| 刷新 | shell32 #16739 |
| 新建文件夹 | shell32 #16804 |
| 剪切 / 复制 / 粘贴 | shell32 #16762 / #243 / #16763 |
| 右侧预览窗 | shell32 #16757（右侧面板图形） |
| Home | shell32 #51380 |
| 下载 / 文档 / 图片 / 音乐 / 视频目录 | imageres #184 / #112 / #113 / #108 / #189 |
| 关闭 | imageres #5102 |
| 搜索 / 重命名 / 删除 | SIID_FIND / SIID_RENAME / SIID_DELETE |
| 桌面或计算机 / 硬盘 | SIID_DESKTOPPC / SIID_DRIVEFIXED |
| 视频 / 图片 / 音频文件备用 | SIID_VIDEOFILES / SIID_IMAGEFILES / SIID_AUDIOFILES |

未发现已验证且含义准确的 back、forward、tabs 图标映射，返回错误触发原有图标。
没有用“撤销/重做”冒充前进后退，也没有用“多窗口”冒充标签页。

## 静态证据与实际验证

2026-10-16 对当前系统做了 REA `inspect_pe_resources` 有针对性库存。
结果保存在工具会话；以下 ID 便于关联证据。静态库存只证明资源存在及
图标组引用，并不能证明图像外观、Windows 加载后的选择或 Explorer 实际使用。

| 目标 | 图标组数 | REA Evidence ID |
| --- | --- | --- |
| System32/imageres.dll | 0 | ev_ec54ff6c8b676c40811a09a17b6b4be27412451cd121b29aed7f2b93889dfd9e |
| System32/shell32.dll | 0 | ev_016311702ed830abd3a0d856bab83aebcf67593a09f24a40a8d89c7947ea2245 |
| Windows/explorer.exe | 23 | ev_5252b6cbf1d2827c49b193e4aa298269a13944fe179b1b9727614e3b89e7333a |
| SystemResources/imageres.dll.mun | 369 | ev_1fe1953beb73bfc0ea0442ea39bf36f69bc51a4e84b98db8b652dded5898c4d9 |
| SystemResources/shell32.dll.mun | 335 | ev_5b3ec565115362e4bc6b60f523bd5f67f0ece2d7b34bf6ba2fbb44a5e39f791c |
| SystemResources/explorerframe.dll.mun | 6 | ev_b4855a5cdc4725f7931d36847857f7936cc98a9f76e18c4013fffea9d6a1df15 |
| SystemResources/ieframe.dll.mun | 102 | ev_d7e20c8b43450b952f8b1dc631602411f99a6c7857d134db6a6c1f6db72d02e6 |

`artifacts/system-icons/contact-sheet.ps1` 按库存中的数字资源 ID
实际加载系统图标并生成接触表。已经检查接触表后才确定以上映射。
这证明当前系统资源外观与所需操作含义相符，未据此声称 Explorer
当前工具栏实际从同一资源 ID 取图，也未修改或再分发 Windows 资源文件。

`examples/system_icons_probe.rs` 实际调用 Rust API：21 类界面图标和
MP4、TXT、PDF、PNG、未知扩展名、EXE 文件关联，以及无映射/超大尺寸的拒绝路径。
每个界面结果均检查存在非透明内容与透明背景。接触表输出到
`artifacts/system-icons/helper-output.png`。首次测量中，单个界面资源
约 0.6–23 ms、文件关联约 4–14 ms；这是当前机器的局部测量，
不是所有机器或 Shell 扩展的延迟保证，调用方仍需超时与失败回退。

黑底与白底两次绘制恢复透明度，兼容较早的 AND mask 图标和较新的 alpha 图标，
避免把透明区域显示成黑色。所有 HICON、HMODULE、DIB HBITMAP、GDI DC
在 RAII 析构中释放，并先将 DC 恢复到原对象。没有使用 `LR_SHARED`，
所以 `DestroyIcon` 只释放本模块拥有的图标。

需要的 windows crate features：`Win32_Foundation`、`Win32_Graphics_Gdi`、
`Win32_Storage_FileSystem`、`Win32_System_Com`、`Win32_System_LibraryLoader`、
`Win32_System_SystemInformation`、`Win32_UI_Shell`、`Win32_UI_WindowsAndMessaging`。
