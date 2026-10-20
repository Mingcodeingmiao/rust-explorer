# Issues #21 / #22 / #23 本地修复与 Win32 API 审查

基线：PR #27 + 已提交的快速访问修复。仅处理本页三项；之前用户直接提出的地址栏、重命名文本菜单、窗口拖动和亮色顶栏修复继续保留。没有加入双栏、顶部收藏栏、空白处双击返回上级或文件夹背景。

## 修复

- **#23 回收站列宽与列选择**：回收站使用独立的 `recycle_columns` 设置，支持拖动调整列宽、列顺序、右键显示/隐藏列和重置。原位置、删除日期支持各自的排序。普通目录的列设置独立；读取配置时校验重复/未知列及非有限宽度。
- **#22 删除确认框乱码**：回收站项目以 `recycle:<PIDL 编码>` 标识，不能把这个内部标识当成文件名。确认框优先使用列表已取得的 Unicode 显示名；缺少显示名时使用通用项目说明，不展示 PIDL。单项、多项确认均使用相同规则，UI 不额外查询 COM。
- **#21 深色全选颜色**：深色选中背景改为 `#36536A`，提高与 `#191919` 背景的区分度；亮色文件行选中颜色未因本 Issue 改动。此前用户指定的文本选中 `RGB(0,120,215)` 与亮色顶栏 `RGB(221,221,221)` 保留。

## Win32 / COM 审查

本轮覆盖 Shell 删除、回收站枚举/属性、菜单初始化及相关资源生命周期；不是对全部第三方代码或所有 Windows API 路径的认证。

1. **修复初始化失败被忽略**：`shell_folder` 的 COM guard 原先把初始化结果转成布尔值，失败后仍继续；部分菜单/驱动器 helper 也使用 `.ok()` 丢弃失败。现要求 STA 初始化成功后才调用后续接口；`RPC_E_CHANGED_MODE` 不再被当成可以继续使用 STA 的情况。成功初始化（含 `S_FALSE`）才生成 guard，并与 `CoUninitialize` / `OleUninitialize` 配对。参考：[CoInitializeEx](https://learn.microsoft.com/en-us/windows/win32/api/combaseapi/nf-combaseapi-coinitializeex)、[CoUninitialize](https://learn.microsoft.com/en-us/windows/win32/api/combaseapi/nf-combaseapi-couninitialize)。
2. **删除操作保持 STA**：`shell::dispatch` 的 `OleInitialize` 结果会传播失败，guard 生命周期覆盖 `IFileOperation`、排队、执行和取消查询。`IFileOperation` 要求 STA，不能仅因线程已有 MTA 就继续调用。参考：[IFileOperation](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nn-shobjidl_core-ifileoperation)。
3. **移除未实现的方法调用**：微软接口文档将 `IFileOperation::SetProgressMessage` 标为未实现；删除流程不再调用它，使用系统进度标题，不把自定义标题设置作为删除的前置条件。
4. **取消与错误处理**：删除同时检查 `PerformOperations` HRESULT 和 `GetAnyOperationsAborted`，不将取消误报为完成。参考：[PerformOperations](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-ifileoperation-performoperations)。
5. **已核对的现有调用**：`SHQueryRecycleBinW` 前设置 `SHQUERYRBINFO.cbSize`；`SHEmptyRecycleBinW` 保留系统确认/进度，空 owner 和空驱动器参数均是可选参数用法。Shell 返回的 `PWSTR` 与 PIDL 在转换/复制后使用 `CoTaskMemFree` 释放。参考：[SHQueryRecycleBinW](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shqueryrecyclebinw)、[SHEmptyRecycleBinW](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shemptyrecyclebinw)。
6. **保留边界**：原生暗色菜单仍包含已有的、按版本检测和缺失回退保护的 `uxtheme` 私有入口。这些不是稳定公开 API，本轮没有借机重写主题实现；后续 Windows build 的兼容性仍需实机验证。

## 验证

- 格式检查、99 项核心测试、1 项独立快捷方式集成测试、全部 targets 编译检查通过；1 项交互删除测试跳过。
- 新增测试：中文/emoji 显示名与 PIDL 隔离、删除日期/原位置排序、独立回收站列配置往返、MTA 线程拒绝 STA guard。
- 独立暗色测试配置下，实测全选高亮、回收站右键列菜单、列显隐、拖动列宽及保存；普通目录列未被改变。
- 仅创建并回收本轮专用中文测试文件；确认框显示原始中文名称，没有内部编码；取消确认后文件仍存在。随后只通过独立 helper 清理本轮专用文件并复查，未清空回收站或删除其他项目。
- 未发布、未上传 GitHub；Windows 10、不同 DPI 和全部第三方扩展组合未在本轮全面复测。

- Release 优化构建通过；本地运行目录：dist/RustExplorer-0.7.6-local-issues21-23（未压缩、未发布）。

## 后续修复：原生菜单线程与重命名双击

- 原生菜单原来直接使用 GPUI 共享后台线程；该线程可能已经是 MTA，OleInitialize 要求 STA，因而报 RPC_E_CHANGED_MODE (0x80010106)。现在每次在独立 STA 线程中构建、显示、执行并释放菜单对象，COM/OLE 初始化与清理配对，不忽略初始化错误，也不强行更改调用线程模式。
- 回归测试在显式 MTA 调用线程中通过相同 STA 执行器创建真实 Shell IContextMenu，确认菜单线程为 STA、调用线程仍为 MTA。100 项核心测试、1 项快捷方式集成测试和全部 targets 检查通过；1 项交互删除测试按原规则跳过。
- 界面实测“显示完整系统菜单”正常弹出并可取消，没有 0x80010106。
- 对照系统资源管理器：进入重命名保留扩展名、默认选中名称主体。按用户明确要求，重命名框内双击选中完整文件名（含扩展名），并避免鼠标抖动立即缩小选区；普通列表的双击打开行为保留。已在中文、连字符和扩展名测试文件中实测，不提交名称修改。
- 参考：[OleInitialize](https://learn.microsoft.com/en-us/windows/win32/api/ole2/nf-ole2-oleinitialize)。未上传 GitHub、未发版。

- 重命名文字菜单按当前选区和剪贴板文本动态生成：无文字选区时隐藏剪切/复制，无可粘贴文本时隐藏粘贴；非空名称保留全选。已在仅有光标时实测菜单只显示粘贴和全选（当时剪贴板有文本）。
