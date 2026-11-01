# Issue #44：悬停时加载圆圈闪现

日期：2026-11-01。对应 [Issue #44](https://github.com/Mingcodeingmiao/rust-explorer/issues/44)。修复保留在本地分支，未发布新版本。

## 问题与原因

报告环境为 Rust Explorer 0.8.0、Windows 11 专业版 26H2、4K/125%。鼠标经过文件夹或普通文件时，都可能反复出现加载圆圈。

悬停提示会通过 `volumes::bounded` 启动 `--file-hint` 进程，图标与缩略图也有独立后台 helper。Release EXE 使用 Windows GUI 子系统。原启动参数只有 `CREATE_NO_WINDOW`，它控制控制台窗口，不会阻止 GUI 进程启动时的反馈光标；短任务退出较快，使圆圈呈现为闪现。

Windows 提供专门的 `STARTF_FORCEOFFFEEDBACK` 启动标志。依据：[STARTUPINFOW 文档](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/ns-processthreadsapi-startupinfow)。

## 修改

- 增加后台查询进程启动器，使用 `CreateProcessW` 和 `STARTF_FORCEOFFFEEDBACK`。
- 接入悬停/目录信息等有超时限制的只读查询、原生图标/预览请求，以及常驻缩略图 helper。
- 保留 Unicode/JSON 参数、管道通信、超时及强制终止；只继承指定的三个标准流句柄，防止其他并发 helper 的管道被错误继承。进程、线程、管道和属性列表通过 RAII 释放。
- 不修改主界面鼠标样式，不取消悬停提示，不修改 owner/STA 菜单分发或文件操作取消路径。

## 验证

- 子进程契约测试直接读取 `GetStartupInfoW`，验证收到关闭启动反馈标志、未收到强制开启反馈标志；同时核对中文、带空格/引号/反斜线的 JSON 参数及 stdin/stdout/stderr。
- 独立测试让子进程阻塞在 stdin 上，验证能检测运行状态、终止并回收进程，保留慢 Shell provider 的超时隔离能力。
- 完整本机检查：163 项核心测试通过；两个忽略项分别是原交互删除测试与供父测试启动的子进程入口。该子进程入口已由契约测试实际执行。快捷方式、原生 OLE 集成测试及全部目标编译检查通过。
- Release 构建通过。Windows 11 深色主题、100% 应用缩放下，文件夹提示正常显示大小及子文件名，普通文本文件的提示和预览正常，中等图标视图能取得文件夹缩略图。该交互检查覆盖了悬停查询、图标请求和常驻缩略图服务的数据通路。
- 独立本地测试程序位于 `dist/RustExplorer-issue44-hover-fix/rust-explorer.exe`，与构建产物 SHA-256 相同；测试使用自建目录和独立设置目录，没有改动默认文件夹关联。

这些测试确认启动参数和通信/取消行为；不将单次截图视为对所有 Windows 版本、DPI 和第三方处理器的逐帧验收。
