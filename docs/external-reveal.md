# 窗口标题与外部定位文件

窗口标题默认显示活动标签页的当前文件夹名称，任务栏悬停标题随目录及标签页切换更新。启用已有的“标题栏显示完整路径”选项时，仍显示完整路径。

外部程序可以调用以下命令打开所在目录并选中目标：

```text
rust-explorer.exe --select "C:\Downloads\example.zip"
rust-explorer.exe /select,"C:\Downloads\example.zip"
rust-explorer.exe --select "C:\Downloads\a.txt" "C:\Downloads\b.txt"
```

多个目标必须在同一文件夹。选中后自动滚动到目标，不进入重命名；显式定位的隐藏/系统文件也可显示。`--open-folder` 或直接传入文件路径时，同样定位该文件；传入目录则直接打开目录。

## 系统边界

默认文件夹关联负责打开目录，不会截获所有软件的“在文件夹中显示”。直接调用 `SHOpenFolderAndSelectItems` 或写死 `explorer.exe` 的程序，仍可能启动 Windows Explorer。当前实现没有全局注入、Explorer 劫持或修改通用 Folder 类，不能承诺浏览器、聊天及下载软件全部转交给 RE。可配置外部文件管理器的软件，应配置上面的 `--select` 命令。

## 本地验证（2026-10-19）

- 88 项核心测试、格式检查及桌面全部 target 编译检查通过。
- 实机创建 181 个文件，定位最后一个文件：正确打开所在目录、自动滚动并高亮，无重命名输入框；原生窗口标题为所在文件夹名称。
- 未更改实际默认文件夹关联，未推送 GitHub 或发布版本。
