# 默认文件夹浏览器（0.7.3 实验性功能）

本功能由 PR #11 的实现重新整合，随后补充命名值备份、系统打开入口兼容、独立恢复进程、Shell 选择桥接和实机测试，随 0.7.3 提供。0.7.2 不包含这些改动。此设置默认关闭，不会因更新或启动自动启用。

## 启用与范围

文件夹选项 → 常规 → **试用默认文件夹浏览器…**。

先将程序及依赖放在固定位置。确认开始后临时启用，再询问是否保留；**120 秒内未确认，独立进程自动恢复原关联**。选择“否”立即恢复。操作与选项底部“应用”按钮独立。

只有备份、应急恢复文件和独立恢复进程都准备就绪后，才写入关联。恢复进程使用单独复制的可执行文件及运行库，不依赖原界面继续运行，不继承启动方的管道或锁句柄；同时采用单调时钟限制试用时间。

仅修改当前用户 `HKCU\Software\Classes\Directory\shell`、`Drive\shell` 下的固定项目，不写 HKLM、不申请管理员权限：

- `RustExplorer.Open`：默认入口、菜单图标、`Document` 多选模型。多选数量仍受 Windows 本身限制。
- `open\command` 默认值及 `DelegateExecute`：兼容外部程序显式调用 `open`，所有原值按原类型、原字节备份。
- `RustExplorer.SystemExplorer`：完整右键菜单中的“在 Windows 资源管理器中打开”，直接启动系统 Explorer。
- `shell` 默认值：选用 RE 的独立 verb。

不接管 Explorer 窗口内部导航、Win+E、控制面板、MTP 等虚拟页面。不修改通用 `Folder` 类，不替换桌面 Shell，不拦截 `explorer.exe`，不安装全局键盘钩子。Explorer 内部双击仍使用自身浏览行为；硬编码启动 Explorer 的软件仍可能打开原版。这不是 Windows 的全局 Explorer 替代。

保留的原生入口也提供恢复通道：即使 RE 原文件被误移动，仍可使用系统 Explorer 找到独立恢复程序。

## 恢复原样

正常方式：文件夹选项 → 常规 → **恢复原样…**，或者运行：

```powershell
& '固定安装目录\rust-explorer.exe' --restore-folder-browser
```

原程序无法运行时，独立恢复程序位于：

```text
%LOCALAPPDATA%\RustExplorer\folder-association\recovery\rust-explorer.exe
```

直接打开它进入选项恢复，或带 `--restore-folder-browser` 参数运行。界面提供 **打开恢复文件夹** 按钮。

最后的应急方案是用 Windows 注册表编辑器导入：

```text
%LOCALAPPDATA%\RustExplorer\folder-association\restore.reg
```

优先使用恢复程序。`.reg` 是备份时的快照，会覆盖对应值，并删除试用新建的具体 verb 键；若后来其他软件修改这些位置，导入前应检查内容。正常恢复程序会保留第三方后来写入的不同值，只删除原来不存在且当前仍为空的键。

备份为同目录的 `backup.json`。重复启用不覆盖最初备份；缺失或损坏备份不猜测原值。失败会尝试回滚，恢复不完整时保留恢复文件并报告错误。恢复成功后保留 `last-restored.json`，解除活动日志。新版兼容上一版六项备份格式。

确认永久保留后，搬移、重命名或删除 RE 前仍须先恢复关联；退出软件不会取消已明确保留的设置。

## 验证记录

以下实机操作执行于 2026-10-18，2026-10-19 继续收尾并复核恢复状态。

- 87 项核心测试、格式检查及桌面全部 target 编译检查通过，优化构建成功。
- 全部 20 个写入步骤逐一注入失败，检查原类型、原字节恢复；覆盖命名值、第三方冲突、备份损坏、恢复保护未就绪、重复启用及旧备份恢复。
- 隔离注册表实际写入、恢复，并用 `reg.exe import` 验证 UTF-16 应急文件。
- `scripts/test_folder_association_guard.py`：启动者退出、原 EXE 暂时移走后仍能超时恢复；确认保留、独立恢复、再次启用和损坏备份保护通过。脚本先验证 debug 隔离根，release 构建会拒绝运行该隔离测试，不会退回真实关联。

用户授权后的真实 HKCU 限时接管结果：

| 操作 | 结果 |
| --- | --- |
| Windows 默认动作打开普通目录 | RE 正常打开 |
| 中文和空格目录 | 路径、列表正确 |
| 系统 `.lnk` 文件夹快捷方式 | RE 打开目标目录 |
| C 盘根目录 | RE 正常显示 |
| 外部显式 `open` 请求 | RE 正常打开 |
| 原生 Explorer 回退入口 | 原版正常打开 |
| Explorer 内部双击 | 保持其自身导航 |
| 独立程序主动恢复 | 原值核对一致 |
| 真实关联 5 秒未确认试用 | 独立进程自动恢复，原值一致 |
| 恢复后 Windows 默认打开 | 原版 Explorer 正常打开 |

初次实测发现 `canonicalize()` 产生的 `\\?\` EXE 路径令 Shell 拒绝访问；已先恢复，再修正为普通 DOS/UNC 命令路径，复测通过。另修复了空键阻止再次启用、子进程继承管道令调用者等待到超时的问题。

本轮核对 Directory、Drive、Folder 的全部非空 HKCU 值，最终保持测试前关联。未测试 Win+E、真实网络共享或手机内部传输；UNC 参数解析由自动测试覆盖。

## Shell 定位桥接（2026-10-19 集成）

默认关联仍使用原来的 `--open-folder` 命令，新增内置 Shell COM 桥接；不增加注册表地址，也不改变旧版恢复日志格式。收到系统选择回调时定位文件，普通目录请求先打开目录并短暂保留接收迟到选择的能力。内部图形入口与外部目录入口分离，避免递归。桥接不可用时回退到直接打开目录。

已通过一次最多 25 秒的真实关联试用：`SHOpenFolderAndSelectItems` 自动启动 RE 并高亮文件；主动恢复后，Directory/Drive/Folder 的完整快照与试用前一致。仍不接管通用 Folder、Win+E 或不支持的虚拟目录；不保证写死 Explorer 的第三方程序兼容。

## 参考

- [File Types](https://learn.microsoft.com/en-us/windows/win32/shell/fa-file-types)：当前用户注册与关联通知。
- [Verbs and File Associations](https://learn.microsoft.com/en-us/windows/win32/shell/fa-verbs)：命令行及引号。
- [Verb selection model](https://learn.microsoft.com/en-us/windows/win32/shell/how-to-employ-the-verb-selection-model)：多选模型。
- [SHChangeNotify](https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/nf-shlobj_core-shchangenotify)：异步刷新通知。
