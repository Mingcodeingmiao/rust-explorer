# 0.6.1 操作与系统菜单复核（2026-10-17）

范围为源码逐项审计、已发布 0.6.1 的只读菜单清单与独立临时文件操作。没有操作 GUI、系统共享权限、格式化、弹出或断开驱动器。结论：不能声称历史需求全部完成。

以下“确定缺失”是对已发布 0.6.1 的初始审计，修复进展见文末，不代表修复后源码仍有相同问题。

## 确定缺失和问题

| 需求 | 结果和证据 |
| --- | --- |
| 全部 Explorer 快捷键，含文件撤销/重做 | **未完成**。src/ui.rs:3353 起无 Ctrl+Z/Y 的文件动作；全 src 没有文件 Undo/Redo 实现。src/shell.rs:196、350、415 的 FOF_ALLOWUNDO/FOFX_ADDUNDORECORD 只是系统操作标记，不等于应用提供撤销/重做。输入框文字撤销也不可替代文件事务。 |
| 自绘“删除访问”和“特定用户”对应不同系统界面 | **未完成**。src/ui_extras.rs:3391 传 remove 区分，但 src/shell.rs:758 的 sharing_access(path,_remove) 忽略 remove，两者同走 ShowShareFolderUI，失败回退 Sharing 属性页。真实系统完整菜单中的同名子项由系统处理，不受此问题影响。 |
| 显示完整菜单包括系统背景菜单全部项目 | **部分完成**。src/shell.rs:286–310 对非空 paths 用真实 IContextMenu，但空 paths 转 native_menu::show 手写背景菜单。src/native_menu.rs:188–284 含查看、排序、分组、刷新、粘贴、新建两项、共享两项、属性；不是完整系统背景菜单，缺系统自定义文件夹等项目，且系统 New 模板未全部接入。若要求所有原生背景项目，这仍需实现。 |
| 驱动器重命名 | **存在源码确定问题，需 GUI 验证并修复**。src/shell.rs:1069 原生 rename 被截回 UI；src/ui.rs:1423 取 path.file_name() 作为初值，盘符根目录结果为空，不能显示卷标。最终 src/shell.rs:415 仍为普通 RenameItem，未有卷标专用编辑分支。建议驱动器 rename 留给原生 Shell，或用卷标读取/验证/更新专用路径；不要以真实系统盘标签变更作自动测试。 |
| 此电脑空白菜单与此电脑对象菜单区别 | **需确认交互**。默认 src/ui_extras.rs:2858 的空白菜单为 view/sort/group/refresh/properties；显式 system_menu 在 src/ui.rs:1330 把此电脑空白映射为 ComputerMenu 对象菜单。二者内容不同，若完整模式也需空白查看/排序，则需保留 app 背景命令。 |
| 文档准确性 | docs/explorer-shortcuts.md 仍写 F6 只有三个区域、导航树键盘未完成；0.6.1 已有独立导航实现，此处文档过时。撤销/重做未完成的描述仍正确。 |

## 已实现，有源码和本次运行证据

- 文件/文件夹/驱动器完整菜单使用真实 Shell IContextMenu：src/shell.rs:938、966 GetUIObjectOf；969 QueryContextMenu(CMF_NORMAL|CMF_CANRENAME|CMF_EXPLORE)；1086 InvokeCommand；690/696 转发扩展菜单消息。没有再用精简自绘菜单代替项目菜单。
- `.tools/verify_full_shell_menus.py dist/RustExplorer-0.6.1-windows-x64/rust-explorer.exe` 本次运行成功。真实清单：普通文件 47 项、文件夹 48 项、C 盘 21 项、此电脑 11 项（含子项，数量受本机扩展影响）。C 盘包含 format、rename、properties；此电脑包含 manage、connectnetworkdrive、disconnectnetworkdrive、delete、rename、properties。此电脑 app 添加的展开/标签/窗口/快速访问/添加网络位置在 inspect 分支之后插入（shell.rs:986 起），因此不在这 11 项中。
- BitLocker、USB 弹出、网络驱动器断开是否出现由真实系统菜单和设备决定；当前清单无 BitLocker，不能虚报该设备功能验收。
- 暗色主题路径 src/native_menu.rs:36–84，对真实菜单 owner 同样调用（shell.rs:924）；版本保护并从 System32 加载 uxtheme。**外观仍需父任务 GUI 观察**。
- 默认完整菜单可在设置切换：settings.rs:70/110（默认 false）；ui_extras.rs:3824/3900；ui.rs:1304 与 ui_navigation.rs:121 路由。
- 左拖/右拖均先创建 Transfer 对话框（ui_extras.rs:673–711）；右拖菜单 Copy/Move/Link/Cancel（2730–2734），选择操作后仍调用 confirm_drop_mode（3234–3246）。确认才发 Request::Transfer（ui.rs:1511）。未使用确认即执行的快捷分支。
- Transfer 实际按卷身份判断，同卷 Auto 移动，跨卷 Auto 复制；Ctrl/Shift/Alt 分别 Copy/Move/Link（ui_extras.rs:742–751；shell.rs:228–241）。会 canonicalize 并拒绝自身/后代（shell.rs:178–192）。
- 本次发布 exe 的独立 Temp 测试全部通过：同卷 Auto 移动、显式 Copy 保留源、Rename 保持文件内容、Link 保留源并产出 .lnk、后代目标拒绝、C 到 D 的跨卷 Auto 复制。文件仅为新建自有测试样本。样本留在 `%TEMP%/RustExplorer-audit-ops-062-4cq7lsng` 和 `.tools/owned-audit-ops-74f7b5dfa1444c17a2185a4b838a703b`。
- 普通删除有 UI 确认和回收标志（ui.rs:1507；shell.rs:393–407）；永久删除有独立确认并拒绝卷根（ui.rs:3205；shell.rs:373–389）。本次未新增回收站污染测试。
- 普通无变化重命名确认可退出：ui.rs:1525–1533 在 close_dialog 后判断同名并 return；修改后由原生 IFileOperation 执行。
- 系统资源导入为持久路径引用（fluent_icons.rs:195–243），已知 shell32/imageres DLL/MUN 映射（254–266），读取时用 LOAD_LIBRARY_AS_DATAFILE|LOAD_LIBRARY_AS_IMAGE_RESOURCE（system_icons.rs:372–375），不执行用户选择的 DLL，不复制系统图标到工程。任意 EXE/DLL 不保证支持未知资源，不能宣称自动获得任意 Windows 版本完整界面。

## 尚需 GUI 的关键证据

暗色原生菜单每级 hover、全菜单鼠标定位和子菜单展开；驱动器 rename 初值和取消；两种右拖确认与取消；默认完整菜单开关的实际鼠标路由。设备敏感操作只检查项目存在和打开前界面，不执行。

## 本轮修复进展

- 驱动器根路径不再使用普通文件名编辑器：新增独立卷标 Dialog；5 秒隔离 helper 读取 GetVolumeInformationW 的原始卷标，确认后才调用 SetVolumeLabelW。只读、网络卷提示清晰；支持空卷标清除，不会把“(C:)”当卷标。两个根路径/卷标纯测试通过；C 盘只读查询得到 Windows-SSD / NTFS，未改真实卷标。GUI 初值及 Escape 由父任务验证。
- 完整背景菜单改为原生 CreateViewObject IContextMenu，保留所有系统扩展与原生 New 模板；仅增加本程序 view/sort/group/refresh/undo/redo。只读比较本机 16 个原生顶项全部保留，合并后 22 项，New 仍有 9 个原生子项。InspectMenu 新增可选 host_commands:true 进行无弹窗验证。
- 普通菜单、导航菜单的驱动器根目录始终走系统专用菜单，无需先开启默认完整菜单设置。
- 菜单 owner 显式透明显示并激活，父 helper 进程取得前台授权，WM_CANCELMODE/跨进程失活结束 TrackPopupMenu；退出后发 WM_NULL 并在 RAII 中销毁 owner/menu/清 IContextMenu。系统 Invoke 的无模式窗口保持 STA 消息循环，避免格式化等对话框刚打开就随 helper 退出。GUI Escape/失活回归由父任务执行。
- 删除访问/特定用户改为两个真实系统子菜单 ID。已确认本机 CommandStore Windows.SharePrivate / Windows.ShareSpecificUsers 的不同 CLSID，通过 GetTitle 与 canonical 只读识别，再匹配已初始化 IContextMenu 的 windows.share 子项，分别是“删除访问”(ID 127)、“特定用户…”(ID 128)。不直接调用私有 IExplorerCommand GetState/Invoke（其缺少 Explorer site 会崩溃），不自行调用 ACL 或 NetShare 修改权限。删除访问不再静默降级为特定用户；实际系统 wizard 只在用户选择菜单时 Invoke，未执行共享变更测试。
- 文件撤销/重做由另一个独立子任务实现，最终覆盖范围和验证请看父任务汇总，本篇不代替其事务测试结果。
