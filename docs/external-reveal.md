# 窗口标题与外部定位文件

窗口标题默认显示活动标签页的当前文件夹名称，任务栏悬停标题随目录及标签页切换更新。启用已有的“标题栏显示完整路径”选项时，仍显示完整路径。

外部程序可以调用以下命令打开所在目录并选中目标：

```text
rust-explorer.exe --select "C:\Downloads\example.zip"
rust-explorer.exe /select,"C:\Downloads\example.zip"
rust-explorer.exe --select "C:\Downloads\a.txt" "C:\Downloads\b.txt"
```

多个目标必须在同一文件夹。选中后自动滚动到目标，不进入重命名；显式定位的隐藏/系统文件也可显示。`--open-folder` 或直接传入文件路径时，同样定位该文件；传入目录则直接打开目录。

## 外部调用响应

显式外部打开请求优先交给同一路径、同一用户配置下已运行的 RE，通过有超时的 `WM_COPYDATA` 消息传递。窗口收到请求后复用对应目录标签页，或新增标签页；最小化窗口会恢复。普通无参数启动仍可新建窗口，不安装驻留服务。

窗口启动过程中会短暂等待其就绪；目标无响应、消息被拒绝或窗口已退出时，回退到独立界面。队列和消息长度均有上限。已有窗口正在重命名或打开选项/对话框时，另开窗口，保留当前输入。

`--open-folder` 已集成 Shell 选择桥：先注册目录以接收系统选择回调，100ms 未收到选择就先打开目录，最多保留 8 秒接收迟到的选择。主界面通过内部 `--browse-folder` 入口启动，避免递归。桥接失败时仍尝试打开目录。图形子进程不继承调用方的标准句柄，防止调用方一直等待输出管道关闭。

这次优化没有扩大默认文件夹关联范围。磁盘、网络和手机目录本身的读取耗时仍然存在；不同权限级别的窗口可能无法互传消息，此时使用回退窗口。

## 系统边界

启用可恢复的默认目录关联后，已实测 `SHOpenFolderAndSelectItems` 从系统启动 RE 并选中目标文件；此前仅支持显式参数的限制已通过 Shell 桥接改善。写死 `explorer.exe`、使用其他接口或不经过当前目录关联的软件，仍可能启动 Windows Explorer。当前实现没有全局注入、Explorer 劫持或修改通用 Folder 类，不能承诺浏览器、聊天及下载软件全部转交给 RE。可配置外部文件管理器的软件，仍可配置上面的 `--select` 命令。Shell 桥接目前只验证单文件选择。

## 本地验证（2026-10-19）

集成后：90 项核心测试、格式及桌面全部 target 检查通过。即时选择、延迟选择、无选择自动退出均通过；一次 25 秒上限的真实 Directory/Drive 关联试用中，系统定位接口成功启动 RE 并高亮目标，之后立即恢复，全部关联快照与试用前一致。原测试版及完整源码备份保留，集成版独立打包，未推送或发版。以下为集成前的历史记录。

- 88 项核心测试、格式检查及桌面全部 target 编译检查通过。
- 实机创建 181 个文件，定位最后一个文件：正确打开所在目录、自动滚动并高亮，无重命名输入框；原生窗口标题为所在文件夹名称。
- 未更改实际默认文件夹关联，未推送 GitHub 或发布版本。

后续响应优化：89 项测试通过，包含真实 Win32 窗口接收、重复请求、拒绝无效路径和窗口销毁后的回退。实机连续 5 次交接约 94–239 毫秒，仅保留一个 UI 进程；最小化后能恢复并高亮；两个同时冷启动请求最终保留一个进程、两个目录标签。时间是调用进程返回时间，不代表目录加载完成时间，不能作为所有机器的性能保证。

另实测以隐藏方式冷启动仍显示窗口并高亮；修复需要在 GPUI 完成初始窗口可见性处理后再显示。重命名期间收到外部请求会在同一进程另开窗口。

REA 已绑定本机 `C:\Windows\explorer.exe`，SHA-256 为 `eee089fe6f927c7f0d51b0abd9d70c094157e33673ad760713ec7b1cea18ee11`。当前未配置 Ghidra 原生分析引擎，Windows 上该 REA 版本的原生 UI 观察工具也不可用；未取得系统 Explorer 内部派发逻辑的反编译证据。上述改动依据本项目源码、实机行为及公开 Win32 消息/前台窗口接口，不宣称复现了系统内部实现。

参考：[WM_COPYDATA](https://learn.microsoft.com/en-us/windows/win32/dataxchg/wm-copydata)、[SendMessageTimeoutW](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendmessagetimeoutw)、[AllowSetForegroundWindow](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-allowsetforegroundwindow)。
