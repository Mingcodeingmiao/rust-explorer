# 实现与验证记录

## 结构

- `src/ui.rs`：GPUI 窗口、标签与导航、虚拟文件列表、多选、拖放、搜索防抖、预览状态和对话框。
- `src/input.rs`：基于 GPUI 官方 Apache-2.0 输入示例改造的输入控件，包含中文 IME、UTF-16 选区与剪贴板文本操作。
- `src/fs.rs`：后台文件枚举、自然排序、名称校验、导航历史。
- `src/search.rs`：Everything 安装与 IPC 检测、后台客户端启动、SDK 异步消息回复、独立查询子进程与超时、当前文件夹筛选降级。
- `src/shell.rs`：OLE、IFileOperation、系统文件剪贴板、IContextMenu2/3、系统已知文件夹和磁盘。
- `src/preview.rs`：预览类型白名单、受限文本读取，以及 BOM/UTF-8/UTF-16/GB18030 自动解码。
- `src/path_completion.rs`：地址栏路径拆分、环境变量展开和目录候选补全。
- `src/content_layout.rs`：内容视图首列（最多 160 逻辑像素，即 175% DPI 下 280 物理像素）、图片中列和日期/大小列的响应式宽度。
- 默认文件夹浏览器以实验性、当前用户可选功能重新整合；原值备份、独立恢复文件、失败回滚与范围限制见 [说明](default-folder-browser.md)。公开 0.7.2 仍为此前暂缓该功能的版本。
- `src/shell_folder.rs`：通过隔离 helper 枚举 Shell 命名空间中的便携设备/MTP 目录，以 PIDL 十六进制路径保存项目标识，并提供 Shell 图标与打开操作。
- `src/native_preview.rs`：Shell 视频单帧和 Windows.Data.Pdf 原生页面渲染。
- `src/native_assets.rs`：带取消、硬超时和 SQLite PNG BLOB 缓存的原生图像 helper 协议。
- `src/system_icons.rs`：系统关联、stock icon 与 DLL/MUN 资源加载，所有 HICON/HBITMAP/GDI 句柄 RAII 回收。
- `src/visual.rs`：串行批量图标加载、DPI 缓存、自绘矢量回退和 rem 布局单位。
- `src/theme.rs`：读取当前用户的 Windows 应用主题。

主进程不加载第三方 Shell 扩展。右键菜单、文件操作、默认应用打开和属性由同一个程序的 `--shell` 子进程处理。Everything 的查询子进程只回传 JSON，主进程在取消或超时后终止它。结果数量限制和文本读取限制均在界面显示。

0.2.0 已完全移除 WebView2/Wry。界面恢复 GPUI 默认 GPU 合成模式；所有预览均为 GPUI 显示的静态图像或文本。视频提取硬限 1.5 秒，PDF 页渲染硬限 4 秒；取消会杀掉 helper。图标由单个后台队列按可见类型分批请求，避免在列表绘制时调用 Shell 扩展。

布局统一以 rem 为单位，在系统 DPI 上叠加应用内 75%–200% 缩放。文件列表默认使用变高虚拟列表，仅测量可见行；启用高性能模式后，详细信息、列表、小图标和平铺视图使用固定可见行组件池：滚动只改变池内项目数据，组件位置和高度保持不动，因此 System32 等大目录不会全量创建数万个组件。分组列表因标题高度不同回退到普通虚拟列表。内容视图使用由 `content_layout` 计算的两列/三列响应式宽度。实际路径和文件操作始终使用原字符串，显示层不再插入零宽断行字符。窗口宽度或缩放变化时失效高度缓存，保持滚动锚点。

## 本机参考证据

2026-10-16，在用户授权下读取系统主题并观察原生资源管理器。

- 注册表：Windows build **26300.9711**，DisplayVersion **26H2**；AppsUseLightTheme 与 SystemUsesLightTheme 均为 0，开启透明效果。
- Computer Use 观察到资源管理器的顶部标签、导航地址与搜索行、独立命令栏、左侧树、详细列表与右侧窗格。布局据此实现。
- REA `inspect_pe_resources` 检查 `C:\Windows\explorer.exe`：PE32+，3,479,440 字节。证据 **ev_5252b6cbf1d2827c49b193e4aa298269a13944fe179b1b9727614e3b89e7333a**；SHA-256 `eee089fe6f927c7f0d51b0abd9d70c094157e33673ad760713ec7b1cea18ee11`。
- REA 的这一操作只证明静态资源身份，不证明界面外观、运行行为或完整功能等价。项目没有复制资源管理器代码；0.2.0 在运行时从当前系统读取图标，映射及视觉验证见 `system-icons.md`。
- Everything 本机版本 **1.4.1.1026**。最初只有 Session 0 服务，应用在搜索时启动用户会话的后台客户端。

## 已执行验证

- Windows GNU 工具链成功编译 GPUI 桌面程序与优化发布程序。
- 五项单元测试：自然排序、大数字名称、Windows 保留/非法名称、历史分支、预览类型白名单与 Everything 搜索作用域。
- `examples/smoke.rs` 在 `.tools/smoke-workspace` 中创建独立测试文件，验证 Shell 复制、重命名、移动；UTF-8/UTF-16 文本；目录枚举；Everything 返回项目的 Cargo.toml。
- 同一 smoke 对 100,000 条模拟记录进行自然排序，debug 构建实测约 34 ms。输入为逆序记录，这是特定数据分布的结果，不是磁盘枚举或资源管理器对比基准。
- 0.1.0 的浏览器视频/PDF 测试记录已被原生实现替代；0.2.0 原生缩略图、PDF 多页与图标的实测数据见 `native-previews.md`、`system-icons.md`。
- `examples/native_asset_probe.rs` 验证应用实际使用的 helper IPC 和 PNG 缓存：本机视频缓存约 48 ms；缺失资源正确回退；模拟卡住的提供器约 1510 ms 被终止；取消约 108 ms 结束。数字是当前机器的样本测量。
- 实际窗口验证了 Ctrl+=、Ctrl+Shift+=、Ctrl+-、Ctrl+0，文字/图标/行高同步变化；长文件名在变高列表内完整换行；系统视频帧与原生 PDF 页面均已显示。原生窗口未启动浏览器子进程。

兼容范围和未实现功能见根目录 README。未进行第三方 Shell 扩展的穷举验证。
