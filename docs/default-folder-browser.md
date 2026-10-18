# 默认文件夹浏览器（本地未发布）

对照 PR #11 的 `f929129` 实现重新整合。0.7.2 整合时曾依照维护者此前暂缓 #10 的要求移除；本次重新实现安全恢复流程，没有替用户启用，也没有更新 GitHub 或发行版本。

## 使用与范围

文件夹选项 → 常规 → **设为默认文件夹浏览器…**。确认后立即生效，不依赖下方“应用”按钮。先将 `rust-explorer.exe` 和依赖放在固定目录，启用期间不要移动、重命名或删除它们。

仅设置当前用户 `HKCU\Software\Classes\Directory\shell` 和 `Drive\shell` 的默认命令。使用独立的 `RustExplorer.Open` verb，不覆盖系统 `open\command` 或 `DelegateExecute`。不接管 Win+E、通用 Folder 类、控制面板、库、MTP 等虚拟命名空间。系统文件对话框以及明确调用 Explorer 的程序不保证改用 RE。

这是实验性 Shell 关联功能，不能称作 Windows 全局替换 Explorer。启用过程校验注册表写入与读回；本轮未更改真实关联，不能据此宣称已实测用户环境中的默认双击派发。

## 恢复原样

正常恢复：同一页面点击 **恢复原样…**。也可以运行：

```powershell
& '固定安装目录\rust-explorer.exe' --restore-folder-browser
```

独立应急恢复文件在：

```text
%LOCALAPPDATA%\RustExplorer\folder-association\restore.reg
```

即使程序无法启动，也可通过 Windows 注册表编辑器导入这个文件。它只恢复上述固定路径的默认值，不递归删除键。**应急导入会恢复备份时的值**；如果之后安装了其他文件管理器，应先检查内容。正常“恢复原样”会保留其他程序后来写入的不同值，并报告冲突。

备份 `backup.json` 保存原始值的类型和字节，在任何关联写入之前同步落盘，同时生成 UTF-16 恢复文件。重复启用不覆盖原始备份。启用中途失败自动逐项回滚；回滚失败保留恢复文件并报告具体错误。缺失/损坏备份不猜测原配置。恢复成功后保留 `last-restored.json` 供检查。原先不存在的值会被删除；可能留下无默认值的空键，避免误删其他程序的内容。

异常退出可能留下待恢复的日志，页面会显示关联变化/未完成，须先恢复后重试。卸载或搬移程序前必须恢复关联。

## 设计依据

- 微软 [File Types](https://learn.microsoft.com/en-us/windows/win32/shell/fa-file-types)：每用户 Classes 注册及关联变更通知。
- 微软 [Context menus](https://github.com/MicrosoftDocs/win32/blob/docs/desktop-src/shell/context.md)：shell 默认值指定默认 verb。
- 微软 [Shell extension registration](https://learn.microsoft.com/en-us/windows/win32/shell/reg-shell-exts)：Directory 与 Folder 范围差异。

## 本轮验证（2026-10-18）

- `scripts/check.ps1`：83 项核心测试通过，格式检查、桌面全部 target 编译检查通过。
- 对每个关联写入步骤注入失败，验证恢复后的原始值（包含原类型）一致；覆盖重复启用不覆盖备份、损坏日志不写注册表、恢复文件生成失败不写注册表、第三方修改冲突保护。
- 在 `HKCU\Software\RustExplorer\AssociationTests` 隔离目录实际写入/读回/恢复，测试结束清理自有测试键。
- 用 Windows `reg.exe import` 实际导入指向隔离目录的 UTF-16 恢复文件，确认无须 RE 即可恢复原值。
- 用 Windows 命令行解析器验证盘符根目录、空格、中文与 UNC 参数；加入 `\.` 避免根路径末尾反斜杠干扰引号。
- 实际启动优化构建，通过 `--open-folder` 打开自有 fixture；“选项 → 常规”的状态、说明、启用/恢复按钮显示正常且没有溢出，取消选项返回文件列表。
- 未点击启用确认、未修改实际 Directory/Drive 关联、未测试真实默认双击派发；未更新 GitHub、未发版。后续启用应由维护者在固定安装位置人工确认。

旧贡献版本如果已经直接覆盖 `open\command`，本实现会拒绝叠加启用，要求先使用旧备份恢复，避免误把旧接管状态备份为“原样”。
