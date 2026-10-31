# Windows 原生文件拖放测试记录

初始实现：2026-10-28，基于 0.7.8；现纳入 0.7.9 part1，见[发行说明](v0.7.9-part1.md)。

## 当前实现

- 使用 Shell 创建的 `IDataObject` 传递多文件数据，包含系统资源管理器等程序使用的 `CF_HDROP` 格式。
- 每个 Rust Explorer 窗口独立注册 `IDropTarget`，根据指针所在目录项、导航项或内容空白区选择目标。放置交给系统目录的 Shell `IDropTarget` 执行，支持不同窗口和与系统资源管理器之间的数据传递。
- 拖动启动投递到窗口自身的 STA 消息循环。GPUI 输入回调返回后进入 `SHDoDragDrop`，避免带着 App/Entity 借用进入嵌套消息泵。
- OLE 拖放循环期间每 30 ms 向源窗口投递一次重绘请求。`SHDoDragDrop` 会阻塞 GPUI 的常规事件循环，没有这个请求时窗口会停在拖动开始前的最后一帧；重绘请求只触发 GPUI 的 `WM_PAINT`，不放宽 STA 或 COM 生命周期约束。
- 跨进程目标（例如系统资源管理器）的 `IDropTarget::Drop` 仍可能同步完成复制。此时 `SHDoDragDrop` 在复制结束前不会返回；源窗口保持重绘，但目标窗口的响应由目标程序自己的 Drop 实现决定。
- 复制、移动、快捷方式由 Shell 协商，图像、光标和落点反馈使用系统拖放机制。
- Shell 优化移动可能返回原始效果 0，但实际已经移动。读取 `Logical Performed DropEffect` 用于刷新，不改写返回给外部源的原始效果，也不重复删除源文件。
- COM 对象、PIDL、窗口子类和 Rust 回调上下文按窗口 STA 生命周期释放，窗口关闭时撤销注册。
- 原有带 owner 的独立 STA 右键菜单路径和可取消文件操作封装未被改写。

## 已完成验证

- `scripts/check.ps1`：格式检查、144 项核心测试（另 1 项忽略）、1 项快捷方式集成测试、1 项原生 OLE 集成测试和全部目标编译检查通过。
- OLE 集成测试使用全新临时目录，验证 Shell 数据对象的 `CF_HDROP`、中文文件名、复制保留源文件、移动移除源文件及目标文件字节内容。
- Windows 实机：单个测试目录拖入目标目录成功；多选两个文档一次拖入目标目录成功；源列表自动刷新。
- 独立测试目录启动成功，无需额外配置 MSYS2 PATH。

## 验证边界与人工测试

Shell 拖放集成测试需要交互式桌面。`scripts/check.ps1` 在本地执行该测试；GitHub 托管运行器使用 `-SkipInteractiveShellTests`，仍编译该测试并显示忽略原因。实际传输在独立子进程执行，60 秒未返回就终止并报告失败，避免阻塞整个 CI。

自动化工具拒绝超出源窗口边界的拖动坐标，因此尚未完成两个可见窗口之间和与系统资源管理器之间的实际鼠标手势验收。底层系统数据对象和 Shell 复制/移动协议已通过测试，不能把协议测试描述成跨窗口实机验收。

运行测试目录中的 `rust-explorer.exe`，按 Ctrl+N 创建第二个窗口，分别打开两个测试目录。测试单文件、多文件和文件夹互拖，再与 Windows 资源管理器双向互拖。核对 Ctrl 复制、Shift 移动、Alt/Ctrl+Shift 快捷方式、Esc 取消及源/目标列表更新。可使用 `.tools/native-drag-fixture` 中的专用文件。

悬停展开、边缘连续滚动、右键跨窗口拖动及不同 Windows/DPI 下的逐像素对齐尚未完成。

实现依据：[Microsoft Shell 剪贴板格式](https://learn.microsoft.com/zh-cn/windows/win32/shell/clipboard)。
