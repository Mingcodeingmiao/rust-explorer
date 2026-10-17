# 0.6.1 导航、此电脑与窗口需求复核（2026-10-17）

范围：只读源码与已有发布目录核对；本报告没有操作 GUI、没有对磁盘/共享/系统设置执行写操作。表中“代码已实现”表示路径与逻辑可追踪，不能替代交互验收。“完成”必须再结合父任务 GUI 证据。

## 修复后复核

下列原始差距 1–4 已修复，原始审计保留为变更前证据，不表示修复后仍缺失。

- 此电脑详情列使用独立 volume_columns，表头与行共享宽度；边缘可拖动、64–1200 限制、释放保存。volume_sort / volume_descending 独立保存，名称按盘符、类型、总大小、可用空间均可点击排序，排序菜单也接入此模型。
- 中、大、超大图标卷名最多四行并省略，选择状态不改变排版。
- 网络卷只读 flags 在原有可终止的 --volume-info helper 中读取，总预算仍为 800ms；UI 未新增同步磁盘/网络查询。未知或超时安全回退，不主动写共享测试权限。卷只读和 ACL 禁写是不同概念，后者未进行写探测。
- 平铺、内容紧凑卡片高度从 76/92 压缩为 64/80；侧栏本身已是普通32、紧凑26，无需重复改动。
- 快访图钉按系统真实 pinned == Some(true) 或本地 favorites 显示；不能判断的频繁访问项不会伪装固定。系统 pin/unpin 后续修复由另一审计分支负责。

验证：cargo check 通过；cargo test --lib --no-default-features 42 项通过，包含新增卷排序及旧配置/异常列宽归一化测试。此分支未操作 GUI，列拖动视觉、四行省略、网络只读真实设备状态仍需父任务 GUI 或设备验收。
## 初次检查的历史差距（修复前）

1. **此电脑详细信息列仍硬编码**：`src/ui_navigation.rs:1119-1157` 每项固定 name=280、kind=160、total/free=130；`1270-1287` 表头同样固定，没有点击排序、过滤箭头或拖动边缘。普通文件页 `src/ui.rs:2405-2414` 调整 `settings.columns`，不会影响此分支。用户要求列宽自由调整；此电脑也需要复用统一的列模型（独立 drive 列配置，支持 total/free），让表头与行使用相同的宽度，最小总宽驱动横向滚动。此项未完成。
2. **网络盘只读状态没有读取**：`src/volumes.rs:79-98` 网络分支只调用 GetDiskFreeSpaceExW 后直接返回；`basic:74` 只有光驱默认只读。因此 `src/ui_navigation.rs:1011-1013` 的隐藏只读进度条条件不能识别只读网络卷。对于 ACL 限制写入的共享还应区分卷只读和用户写权限，不应默认当作可写。建议在隔离 helper 有界查询 GetVolumeInformationW 卷 flags；查询失败标记未知，不做主动写探测。
3. **此电脑中/大/超大图标的名称仍全部单行省略**：`src/ui_navigation.rs:1028-1035,1161-1185` 三种网格模式沿用同一 nowrap label，没有最大四行逻辑。用户最新要求图标模式文件名最多四行；这条是否包含卷名仍需明确，但旧截图要求此电脑网格文本在图标下有多行。建议共享图标标签排版，保持选择后不展开全部。
4. **此电脑平铺的紧凑模式不改变卡片高度**：`src/ui_navigation.rs:1040-1064` 非 details/small 的平铺固定 min_h=76，而 compact 仅改变 details/small。原生截图对紧凑布局要求较强；需 GUI 比对再决定卡片和分组间距。
5. **快访固定/取消固定是应用层覆盖，不会更改系统固定列表**：`src/ui_navigation.rs:306-333` 合并系统快访与本应用 favorites，过滤 hidden；`src/ui_extras.rs:3420-3446` 取消固定记录 hidden、本地添加记录 favorites。系统轮询确实存在，但用户如果预期和 Windows 双向同步，目前没有系统 pin/unpin 调用。属于行为边界，不应声明完整系统快访管理。

## 初次检查矩阵（修复前，当前状态以上方修复后复核为准）

| 需求 | 代码核对结果 | 证据与仍需验收 |
|---|---|---|
| 此电脑默认平铺、类型分组 | 代码已实现 | `src/ui.rs:487-498,525-530,732-748`；允许已记住的查看方式覆盖默认，进入此电脑重设 kind 分组。需清洁配置启动验证。 |
| 此电脑按盘符而非标签排序 | 代码已实现 | `src/ui_navigation.rs:438-450,1225-1235` Name 比较 path，descending 会反转；`src/volumes.rs:45` 根列表排序。需自定义卷名反序的 GUI 核对。 |
| 8 种查看模式 | 分支齐备，未全部 GUI 验收 | `src/settings.rs:5-37`；`src/ui_navigation.rs:984-999,1213-1403`。三种大图标同布局不同尺寸，Small 横排多列，List 每组一列，Details 四列，Content 整行，Tiles 多列。 |
| 此电脑 List 无分组一列、有分组每类型一列 | 代码已实现 | `src/ui_navigation.rs:1221,1256,1289-1311,1366-1389`；List 分组标题双击不折叠，单击选组 `1329-1350`。需多设备窄窗确认横滚。 |
| 此电脑详细信息列自由调整 | **未完成** | 见上方差距 1。 |
| 此电脑分组标题整行选择/双击折叠 | 代码已实现 | `src/ui_navigation.rs:1308-1351`；标题 w_full，单击加入成员，双击非 List 切换 device_groups。 |
| 进度条未用空间浅色、增加厚度 | 代码已实现，像素未验收 | `src/ui_navigation.rs:1188-1204` 背景 e6e6e6、border bcbcbc，Tiles 16、Content 20 逻辑像素。尚无与本机原版同 DPI 测量证据。 |
| 0 B 可用/只读不显示条 | 本地卷代码实现，网络只读有缺口 | `src/ui_navigation.rs:1011-1013`；本地 flags `src/volumes.rs:121`，网络缺口见差距 2。 |
| 网络盘使用率条 | 代码已实现，需设备证据 | `src/volumes.rs:79-98` 有界辅助进程查询容量；`src/ui_navigation.rs:1188` 不排除 kind=4；离线失败显示未知，未用零容量伪造。 |
| 系统驱动器 Windows/BitLocker 图标 | 系统加载路径存在，需实际锁定卷验收 | `src/ui_navigation.rs:1160` 调 visual::file_icon_sized(volume.path,true,...)，不以普通文件夹 stock 图标替换。系统覆盖图标应由系统提供。 |
| 主文件夹、快访、树横线 | 代码已实现 | `src/ui_navigation.rs:637,677-689` 插入空行 separator；`791-813` 绘制 1px 横线。 |
| 快访一层可折叠 | 代码已实现 | `src/ui_navigation.rs:639-675` 根 expandable=true，孩子 expandable=false，Collapsed marker 控制一层。 |
| 快访动态查询刷新 | 代码已实现 | `src/quick_access.rs:38-87` 枚举 Shell Quick Access 真实路径；`src/ui_navigation.rs:274-304` 15秒节流、1600ms helper 超时。`src/ui.rs:632` 周期调用。不能据此宣称系统 pin/unpin 写回。 |
| 快访文件夹可打开、有空白右键 | 代码已实现 | sidebar `src/ui_navigation.rs:937-945` navigate；快访主页卡片 `1481-1562` 选择并 open_selected；背景 `1414-1436` 右键 custom quick-background menu。 |
| 快访图钉靠右、整行可点 | 代码已实现 | `src/ui_navigation.rs:842-852,916-929,930-950` row w_full，名称 flex_1/min_w_0，pin 在最后，点击绑定 row。需窄窗防溢出检查 mx_1。 |
| 此电脑/桌面/主文件夹/快访图标分开 | 代码已实现，视觉需验收 | `src/ui_navigation.rs:622-644,904-913` 分别 home、desktop、quick-access、computer-colored；运行时 native 资源缺少时走自绘回退，不能声称所有资源都是原生 SVG。 |
| sidebar 拖宽、预览拖宽与保存 | 代码已实现 | `src/ui.rs:2359-2365,2396-2403,2470-2478,2884-2919` 两条 5px divider，140..600 / 180..900 夹紧，mouseup 保存。需鼠标离开窗口/释放事件 GUI 验收。 |
| 回收站入口 | 代码已实现（系统窗口） | `src/ui_navigation.rs:632-636`；`src/ui.rs:726-730` Shell Open，不在本应用内部渲染。用户仅要求快速入口，此范围满足。 |
| 地址过长显示三点、弹出祖先路径 | 代码已实现 | `src/ui_extras.rs:905-1029` 实际 shape_line 估宽、折叠祖先；`2744-2753` 祖先菜单。需长当前目录、窄地址栏实测菜单锚点。 |
| 工具栏指定顺序收纳到三点 | 代码已实现 | `src/settings.rs:169-194` 顺序 view,sort,delete,share,rename,paste,copy,cut；`src/ui.rs:2579,2641` 省略；`src/ui_extras.rs:2928-2968` 三点重建对应项目。固定成本估算仍需各 DPI GUI 验证。 |
| 新建后、查看与三点之间竖线 | 代码已实现 | `src/ui.rs:2631-2635,2644-2646`。 |
| 亮/暗/跟随系统，默认跟随 | 代码已实现 | `src/settings.rs:109`；`src/theme.rs:16-30`；选项 `src/ui_extras.rs:3854-3868`；周期检测 `src/ui.rs:689-695`。需不修改系统配置情况下检查两种主题，跟随系统仅代码证据。 |
| DLL 资源选择、避免复制系统资源到项目 | 选择引用已实现，非任意完整资源兼容 | `src/ui_extras.rs:3846-3853`；`src/fluent_icons.rs:171-197` 仅已知文件名/资源映射可以供图标，不能承诺任意 Win11 DLL 的完整体验。 |
| 选项滚动条、无滚轮可用 | 代码已实现 | `src/ui_extras.rs:3771-3778,3980-3987` ScrollHandle + 独立可拖滚动条；overlay 处理 drag `3930-3948`。需 GUI 拖动验收。 |
| 打开选项不能操作主窗口 | 鼠标遮挡和多数快捷键保护已实现，需 GUI | `src/ui_extras.rs:3925-3949` absolute/inset/occlude、左右键 stop；`src/ui.rs:1976,2037,2089` 多处 options guard。全局 CtrlTab/Zoom 是否被所有 guard 截断需父验收。 |
| 窗口按钮恢复后不消失 | 布局防护已实现，需多次切换验收 | `src/ui_extras.rs:1650-1699` 控件 absolute/right/138宽；需最大化/还原两轮，不检验用户明确跳过的边缘贴靠。 |
| tab 不占满，留拖窗区 | 代码已实现 | `src/ui_extras.rs:1514-1538,1633-1647` 预留300，caption drag至少64，控件138；小窗大量 tab 需 GUI 检查是否挤出或被覆盖。 |
| tab drag ghost 和 tab 等尺寸 | 逻辑尺寸已一致，DPI需验收 | `src/ui_extras.rs:1546-1578` tab宽 tab_width/高36，TabGhost `258-280` 同宽高36。 |
| 关闭 tab 默认不红，仅 hover | 代码已实现 | `src/ui.rs:1848-1878` 通用按钮只 hover t.hover，未赋红底；窗口 close hover 红 `src/ui_extras.rs:1690-1697`。 |

结论：本范围不能宣称所有历史需求全部完成。最明确未完成的是此电脑专属详细信息的可调整列；网络只读判断存在实质缺口。GUI 外观与设备相关状态请结合父任务测试，避免将源码存在等同通过验收。


## 后续撤销/重做任务

本分支还完成了真实回执驱动的复制、移动、重命名、新建、回收站删除 Undo/Redo 核心，并由父任务接入 Ctrl+Z、Ctrl+Y 和菜单。完整实现、API、部分失败处理、覆盖/合并等不可撤销屏障、身份和冲突检查及自有 Temp 测试证据见 [file-history-062.md](file-history-062.md)。47 项库测试通过；GUI 接线验收仍由父任务执行。此处不再把完整文件操作 Undo/Redo 归为“完全没有实现”，但不会声称覆盖和目录合并可以恢复被覆盖内容。
