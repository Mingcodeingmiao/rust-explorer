# Contributing / 参与贡献

欢迎报告问题、改进文档或提交小而清晰的 Pull Request。项目目前面向 Windows 10/11 x64，仍在迭代中。

> [!CAUTION]
> **右键菜单性能红线：不得把带主窗口 owner 的菜单改回“一次性 helper + 可取消操作”，不得清零 owner。**
> 这会导致每次右键启动进程并冷加载 Shell 扩展，曾造成接近一秒的延迟。PR #30 已恢复进程内独立 STA 菜单路径。删除/复制等文件操作仍使用可取消 helper；不要再次把两类请求合并。详见 [维护约束](AGENTS.md)。

## 开发环境

1. 安装 Rust stable、Visual Studio C++ Build Tools 和 Windows SDK（含 `fxc.exe`）。Rust 2024 edition 至少需要 Rust 1.85；实际构建依赖以当前 stable 和 `Cargo.lock` 为准。
2. 运行 `powershell -ExecutionPolicy Bypass -File scripts/build.ps1`。Everything SDK 是可选项；需要时先运行 `scripts/bootstrap.ps1` 下载官方 SDK。
3. 检查代码：`cargo fmt --all -- --check`、`cargo test --locked --lib --no-default-features`、`cargo check --locked --all-targets`。可使用 `scripts/check.ps1` 顺序执行。

GPUI 是桌面默认 feature；无默认 feature 的库测试可检查核心逻辑，而不启动界面。项目不是跨平台文件管理器，请勿把 Windows 专有代码的 Linux 构建失败当作支持回归。

## 提交要求

- 从 `main` 创建分支，每个 PR 解决一个明确问题；说明触发条件、修改行为和验证结果。
- 文件操作、Shell helper、快捷方式、回收站及撤销变更必须使用新建的独立测试目录，不要使用个人文件。不要自动执行格式化、权限变更或设备断开来测试菜单。
- 保持磁盘枚举、缩略图和扩展加载在后台或隔离 helper 中；不在渲染路径做同步网络/磁盘查询。
- **⚠️ 右键菜单不得改回"一次性 helper + 可取消"的请求路径。** 菜单必须保留 `owner` 并在应用进程内构建、显示（`execute_menu_in_process`）。改成一次性 helper 会让每次右键都新起进程并冷加载 Shell 扩展：实测从 **40–80 ms 退化到 ≈250 ms，扩展多时接近 1 秒**。菜单是"点一下就关"的交互，不需要 `OperationControl` 的取消语义；该语义只用于删除/复制/格式化等长操作。`src/shell.rs::execute_controlled` 顶部有同样的警告注释，`context_menus_keep_an_owner_and_stay_out_of_the_one_shot_helper` 是守卫此约束的回归测试。
- 不提交系统 DLL、提取的 Windows 图标、个人路径、日志、凭据、安装包或缓存。新增依赖应说明必要性和许可证。
- 涉及 UI 的变更附截图、Windows 版本、DPI/缩放、主题和查看模式；发布前测试最大化/还原、窄窗口、键盘操作及失败回退。
- 标注实际测试范围；源码存在、构建成功和交互验收是不同的证据。

提交贡献即表示你有权提交这些内容，并同意将其按本项目 Apache-2.0 许可分发。请保持尊重，遵守 [行为准则](CODE_OF_CONDUCT.md)。
