# Shell 文件定位桥接实验

这是参考 Files 的独立实验，不参与主程序 Cargo 构建，不覆盖现有测试包，不安装默认关联。调查和实测日期为 2026-10-19。

后续集成已把共享 COM 实现移到 `native/shell_bridge/`，MIT 许可移到 `resources/files-MIT.txt`。主程序的 `--open-folder` 已静态链接并调用此桥接，实验启动器和探针仍可独立复测。以下为最初独立实验记录；集成行为见 [外部定位说明](../../docs/external-reveal.md)。

## 备份与安全边界

实验前已将提交 `d1618b12e3b3a664c284914c0f5f06568fbc45fc` 的源码 ZIP、完整 Git bundle、可运行预览目录、当前 Directory/Drive 注册表导出及 SHA-256 清单保存在本地 `artifacts/before-shell-bridge-20261019-101254/`。当时 HKCU 的 Folder 类不存在，未创建或导入它。bundle 已验证完整；备份 EXE 与当前预览 EXE 哈希相同。

备份预览目录可直接运行；若要查看旧源码，可将 bundle 克隆到另一个新目录，避免覆盖当前工作。没有真实关联写入需要回滚，实验桥接正常退出或终止后均验证了目标目录的 Shell 注册不再可查找。

测试只在新建的临时目录创建文件，读取当前关联快照，不使用旧任务的关联快照恢复本机。测试前后的旧快照可能因用户操作而不同，因此本轮以实验开始时的状态为准。

## 原理及适配

`OpenInFolder.*` 改编自 Files 提交 `6b721e819e4006670c5926325450fef9157aa6b4`，保留 Files Community 版权及完整 MIT 许可。移除了 WIL/WinRT 构建依赖，以普通 COM 和本地 RAII 对象替换；输入改为显式路径，避免固定长度参数缓冲区。Launcher 和测试探针为本实验代码。

1. STA 进程创建隐藏工具窗口，通过 `IShellWindows.RegisterPending/Register` 注册目录，暴露 `IWebBrowserApp/IServiceProvider/IShellView`。
2. 外部探针实际调用 `SHOpenFolderAndSelectItems`，而不是直接调用桥接对象的 `SelectItem`。
3. Shell 找到对应目录窗口，回调 `IShellView.SelectItem`；桥接器合并相对 PIDL，向现有 RE 发送 `--select`。
4. 500ms 没有选择就先打开目录，仍保持消息循环，最多等待 10 秒接收延迟选择；结束后撤销 Shell 注册。

关键适配：直接按参考代码使用 PIDL 字节数组注册时，本机能枚举窗口，但 `FindWindowSW` 返回 `S_FALSE`，没有选择回调。改用路径 BSTR 注册并通知导航后，`FindWindowSW(SWC_BROWSER)` 返回 `S_OK`，原生选择调用及 RE 高亮均通过。不能据此断言参考实现对所有 Windows 版本均有问题。

## 构建和复测

需要 Windows、现有的 MinGW-w64 `g++`、Python，以及本地 `dist/RustExplorer-default-browser-preview/rust-explorer.exe`。将工具链目录加入当前进程 PATH，再运行：

```powershell
./experiments/shell-bridge/build.ps1
python experiments/shell-bridge/test_bridge.py immediate
python experiments/shell-bridge/test_bridge.py delayed
python experiments/shell-bridge/test_bridge.py no-selection
python experiments/shell-bridge/test_bridge.py aborted
```

输出在 `artifacts/shell-bridge-experiment/`。各模式生成自己的临时目录、隔离 RE 配置和 JSON 结果；会打开测试窗口，不会更改默认关联或删除用户文件。桥接程序必须按调用者指定的目录注册，故本实验不能让未接入桥接器的软件自动调用 RE。

## 实测结果

| 用例 | 结果 |
| --- | --- |
| 中文、空格、逗号文件名，即时系统选择 | `READY → DIRECTORY → SELECTED`；RE 显示目标高亮；本轮约 1.5 秒完成桥接及探针流程 |
| 延迟 1.2 秒才发出系统选择 | 先打开目录，再选中文件；约 2.7 秒，包含刻意等待时间 |
| 不发出选择 | 打开目录；约 10 秒自动结束，事件 `NO_SELECTION` |
| READY 后强制终止 | 退出后目标目录的 Shell 注册不可查找；系统关联快照一致 |
| 不存在的目录 | 返回错误码 5，不打开 RE |
| 缺少参数 | 返回错误码 2 |

所有四个主用例的 `registry_unchanged` 和 `registration_revoked` 均为 true。界面验证使用实际 RE 窗口；整体时间包含探针查找、跨进程 COM、进程启动等，不是界面绘制耗时指标。

## 尚未验证或启用

未修改通用 Folder、Win+E 或任何现有默认设置；未验证所有浏览器/下载/聊天软件、多个同时选择的文件、网络路径和 MTP；目前只验证单个文件定位。没有将实验纳入设置开关，也没有推送或发版。

下一阶段应先做轻量启动桥与 RE 恢复日志的正式集成，再为任何新增关联地址提供限时试用及独立恢复。此次不将单次实验成功当作全局接管已完成。
