# Files 默认文件管理器实现对比

调查日期：2026-10-19。仅阅读源码、文档并记录方案；未运行 Files、导入其注册表脚本、修改系统关联或迁移实现。

## 结论

Files 有默认文件管理器功能，位于高级设置，并被官方文档列为实验功能。值得借鉴的关键是原生启动桥接器和 Shell 选中文件接口，而不只是扩大注册表接管范围。RE 当前的可恢复关联与窗口复用可以保留；通用“显示所在文件夹”还需要另一层兼容桥接。

本次核对上游 main 的固定提交：`6b721e819e4006670c5926325450fef9157aa6b4`。以下是源码观察，不代表已在这台电脑安装并验证了 Files 的跨软件兼容性。

## 已确认的实现

| 功能 | Files 源码机制 | 对 RE 的价值 |
| --- | --- | --- |
| 默认目录打开 | `SetFilesAsDefault.reg` 改写 HKCU 下 `Folder\shell\open\command` 和 `explore\command`，清空 `DelegateExecute`，指向独立 `Files.App.Launcher.exe` | 接管面比 RE 的 Directory/Drive 大，须单独评估虚拟目录和回退 |
| Win+E | 改写 CLSID `{52205fd8-5dfb-447d-801a-d0b52f2e83e1}` 的 `opennewwindow\command` | 是另一个独立入口，不必为了文件定位同时启用 |
| 外部文件定位 | 原生 Launcher 建立隐藏工具窗口；`OpenInFolder` 实现 `IWebBrowserApp`、`IServiceProvider`、`IShellView`，向 `IShellWindows` 注册目录；`SelectItem` 收到相对 PIDL，合成完整目标后转发 `-select` | 可接收一部分 `SHOpenFolderAndSelectItems` 的选中请求，补足仅靠启动参数无法获得目标文件的问题 |
| 异步请求 | Launcher 初次等待 500ms；若没有目标，先激活目录，再保留消息循环最多 10 秒，接收迟到的选择并补发；协议激活另有输入就绪/宽限等待 | 必须处理“先打开目录、后收到选中文件”，不能第一次超时就退出；具体时限不应机械照搬 |
| 已运行实例 | `Program.cs` 使用 Windows App SDK `AppInstance` 重定向激活，检查是否误指向自身，并有缓存实例唤醒路径 | 与 RE 新增的已有进程接收请求方向一致；RE 使用 GPUI，不能原样复制 WinUI/App SDK 生命周期 |
| 原生 Explorer 回退 | Launcher 对不支持的 Shell 目录调用 Explorer；还尝试复用已有原生 Shell 浏览窗口 | 接管后仍要保留控制面板等系统页面，不应把所有 Shell 地址当普通路径 |
| 恢复 | 提供固定 `UnsetFilesAsDefault.reg`；Launcher 检测应用别名消失后尝试恢复 Explorer | 恢复入口值得保留，但 RE 应继续按启用前原始值恢复，而非直接覆盖为固定模板 |

## 建议借鉴的范围

1. 独立实现可测试的 Shell COM 定位桥，将收到的目录和目标文件交给 RE 已有请求通道。先在隔离条件中验证注册、回调、撤销及延迟选择，不先改真实 Folder 关联。
2. 如需更快冷启动，可把轻量启动桥与完整 GPUI 界面分离；已有窗口优先接收请求。后台常驻只作为另一个显式选项，不作为本轮默认行为。
3. 保留 RE 的原始值备份、限时试用、独立恢复程序、失败回滚和第三方变更保护。扩大 Folder 或 Win+E 接管时，为新增地址建立相同恢复记录。
4. 区分普通目录、可支持的 Shell 目录与必须由 Explorer 处理的目录，避免关联递归及错误吞掉系统操作。

Files 的桥接提供了进一步改善兼容性的源码依据，因此无需因为“没有参考实现”放弃此功能。但它不是 Windows 官方默认文件管理器切换 API，也不能从源码推断所有浏览器、聊天、下载软件必然兼容。当前没有将这些扩大接管的改动应用到 RE。

## 与当前 RE 的区别

RE 当前有 Directory/Drive 的可恢复关联、`--select`/`/select,` 参数、窗口复用、高亮及滚动；尚无上述 Shell COM 选择桥，也没有接管通用 Folder 或 Win+E。本轮研究不改变这些范围。

## 来源与许可

- [官方高级设置说明](https://files.community/docs/customize-settings/advanced)
- [SetFilesAsDefault.reg](https://github.com/files-community/Files/blob/6b721e819e4006670c5926325450fef9157aa6b4/src/Files.App/Assets/FilesOpenDialog/SetFilesAsDefault.reg)
- [UnsetFilesAsDefault.reg](https://github.com/files-community/Files/blob/6b721e819e4006670c5926325450fef9157aa6b4/src/Files.App/Assets/FilesOpenDialog/UnsetFilesAsDefault.reg)
- [FilesLauncher.cpp](https://github.com/files-community/Files/blob/6b721e819e4006670c5926325450fef9157aa6b4/src/Files.App.Launcher/FilesLauncher.cpp)
- [OpenInFolder.cpp](https://github.com/files-community/Files/blob/6b721e819e4006670c5926325450fef9157aa6b4/src/Files.App.Launcher/OpenInFolder.cpp)
- [OpenInFolder.h](https://github.com/files-community/Files/blob/6b721e819e4006670c5926325450fef9157aa6b4/src/Files.App.Launcher/OpenInFolder.h)
- [Program.cs](https://github.com/files-community/Files/blob/6b721e819e4006670c5926325450fef9157aa6b4/src/Files.App/Program.cs)
- [AdvancedViewModel.cs](https://github.com/files-community/Files/blob/6b721e819e4006670c5926325450fef9157aa6b4/src/Files.App/ViewModels/Settings/AdvancedViewModel.cs)

已读 Launcher 及相关 C# 文件头标注 MIT。若后续复制或改编代码，须保留对应版权和 MIT 许可说明；本次只记录架构分析，未将上游代码并入 RE。
