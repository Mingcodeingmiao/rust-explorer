# 文件显示与交互复核（0.6.1，2026-10-17）

范围：只读源码审计和隔离命令行测试；没有操作 GUI。以下“已实现”仅表示存在对应实现路径，不能代替交互验收。

## 实际运行的验证

针对发布文件 `dist/RustExplorer-0.6.1-windows-x64/rust-explorer.exe`：

- `py -3.11 .tools/verify_shortcut_thumbnails.py ...`：通过 6 项真实系统快捷方式测试：照片/视频/PDF .lnk、本地 file URL、目标文件变更导致缓存失效、不支持目标回退系统图标。
- `py -3.11 .tools/verify_font_previews.py ...`：通过 Segoe UI TTF 和 Microsoft YaHei TTC 的系统字体原位只读预览、缩略图、缓存与源文件未修改检查。
- 默认 `py` 指向 Python 3.13，缺少 Pillow，最初两次测试未执行；改用已有 Pillow 的 Python 3.11 后成功。未安装依赖、未复制系统字体。

## 逐项对照

| 用户需求 | 源码结论与证据 | 还需要的验收 |
|---|---|---|
| 重命名后名称未改变，点空白退出 | 已实现。`src/ui.rs:2815` 空白先 confirm；`src/ui.rs:1486` 先 close_dialog 再执行分支；`src/ui.rs:1524` 同名直接返回 | F2 后直接点空白；确认没有错误框、输入框消失 |
| 重命名框随名称长度伸缩、过长完整显示 | 已实现。`src/ui_extras.rs:1868` 按 shaping 宽度、最多 520 逻辑像素软换行；deferred anchored overlay；`src/input.rs:550` WrappedLine；`src/ui.rs:462` Changed 刷新父界面 | 英文/中文长名、窄窗、靠右列、输入法、删除文字后缩小；注意高度计算使用 20，而输入控件紧凑行高是 18，可能多余留白，不等同于裁剪 bug |
| 详细信息重命名时行高不变 | 已实现。`src/ui_extras.rs:1909` overlay 占位固定 22；`src/ui_extras.rs:2112` Details 固定 26/34 行高 | 纵向行位置与列对齐前后比较 |
| 中/大/超大名称最多四行，选中也不能展开 | 已实现。`src/ui_extras.rs:1943` 模式决定 wrap，不依赖选中；`:1948` line_clamp(4)；`:2053` 同步测量最多四行高度 | 中文、无空格英文长名、选中前后与缩放 |
| 名称/文件类型截断、不意外双行 | Details/List/Small 单行实现于 `src/ui_extras.rs:1949`；类型列 `:2330` nowrap+ellipsis；Tiles/Content 信息 `:2439` nowrap | 窄列下省略号与编辑覆盖 |
| 图标高亮按各项目内容大小计算 | 已实现。`src/ui_extras.rs:2045` 单项宽；`:2053` 单项名称行数；`:2474` 外层网格分配与内层高亮尺寸分离 | 同行短名/长名不同框宽高，点击空槽不要误选 |
| hover/选中/选中+hover 三态 | 已实现。`src/ui_extras.rs:2126` 基础 selection；`:2131` hover 根据 self_selected/drop_target 取更强色 | 深/浅色和拖拽状态颜色对比 |
| 取消选择保留虚线，多选第一个虚线，再选别项清旧虚线 | 已实现。`src/ui.rs:971` focus_item 更新；`src/ui_extras.rs:2459` focus_outline 条件；`src/visual.rs:356` 虚线绘制 | Ctrl 取消、空白取消、多选与组排序下“第一个”匹配预期 |
| 按空白拖动框选 | 已实现。`src/ui.rs:299` begin_marquee，`:332` 边界相交；`:2337` 鼠标移动，`:2416` 释放；`:3303` 可视框；Details 空白入口 `:2815` | 正反方向、Ctrl/Shift、空白点击、水平滚动后选区位置。没有看到框选贴边自动滚动；用户没有明确提出该扩展 |
| 列表横向滚动，详细信息普通滚轮不得同时横向走 | 已实现。`src/list_layout.rs:13` 按列打包；`src/ui_extras.rs:533` 只 List 或 Shift 转横向；`src/ui.rs:2795` restrict_scroll_to_axis | 宽 Details 纵向滚轮的 x offset 不变；List 纵轮变横向；Shift 有意横向 |
| 列宽手动调整，表格总宽不强制填满；双向滚动条 | 已实现。`src/ui.rs:2404` 宽度拖动；`src/ui_extras.rs:1689` 表头；`src/ui.rs:2789` 横向视口；`:2854`/`:2855` 两条自绘滚动条 | 窄窗/总列宽小于窗/超宽列，拖到边缘不遮最后项 |
| 名称/日期/类型/大小列筛选 | 已实现。`src/file_filter.rs:16` 名称含拼音且补齐 M–S；`:27` 日期；`:33` 大小；`:149` 跨列 AND/列内 OR；`src/ui_extras.rs:1796` 下箭头及 `:2691` 菜单 | 表头打开筛选、组合筛选、清除和空结果显示；今天/未知日期没有单独选项，但符合用户明确列出的四档日期范围 |
| 分组整行选择，双击折叠/展开；List 不双击折叠 | 已实现。`src/ui_extras.rs:1232` select_group；`:1257` List 禁折叠；`:2572` group w_full，图标随 collapsed 更新 | 分组整行命中、全选组内可见项、折叠后再展开。组标题本身没有持久 selection 背景，仅 hover，若需要标题保持选中色需另补状态 |
| 隐藏项目半透明 | 已实现。`src/ui_extras.rs:2135` 对整个条目 opacity(0.5) | 各 view 深浅色 |
| .lnk/.url 与 junction/目录符号链接箭头；云占位别误标 | 已实现。`src/fs.rs:95` 只 tag 0xA0000003/0xA000000C；`src/visual.rs:279` 小图 overlay；`src/ui_extras.rs:2395` 图标模式 overlay | 真正 junction 权限拒绝与 shortcut fallback 图标仍需 GUI；不能仅凭图标缓存成功就认定箭头显示 |
| 文件夹快捷方式双击本程序导航 | 已实现。`src/ui.rs:1092` open_entry，`:1123` 1500ms 隔离解析；`src/open_target.rs:29` 最多八跳、目录 canonicalize；支持本地 URL；特殊 Shell/网页仍交系统 | 真 .lnk 双击不启动 Explorer、相对路径与链；helper 分类不等于双击流程实测 |
| 字体双击 Windows Font Viewer | 已实现。`src/ui.rs:1141` FontPreview；`src/open_target.rs:102` 明确 System32/fontview.exe。字体 thumbnail helper 实测通过 | GUI 双击是否打开 Font Viewer，不点击安装 |
| 图片/视频/链接/文件夹/字体缩略图，中图标及以上 | 已实现，有发布 helper 证据（上方）；`src/ui_extras.rs:2358` 仅 large 模式请求；`src/thumbnails.rs` 异步缓存 | 快速切视图和大目录响应、慢网络不阻 UI，需要实际性能检查 |
| 小缩略图最小 1/9 面积居中加框，中等原尺寸，过大缩小 | 已实现。`src/icon_layout.rs:2` edge clamp(logical/3, logical)；`src/visual.rs:323` 居中与边框；legacy 低分辨率 canvas 修整 `src/icon_layout.rs:13` | 极小、横图、竖图、原生不透明 canvas |
| 竖图不压文字 | 已实现。`src/visual.rs:310` 按宽高比显式 w/h，外层固定 icon slot | 竖图下四行名，在各 DPI/zoom 无重叠 |
| 视频电影胶片效果和右下角播放器标识 | 存在实现。`src/ui_extras.rs:2380` film_strip + 20px file_icon_sized；`src/visual.rs:387` film strip | 当前右下角是该视频文件的系统关联图标，不保证系统返回播放器 app 自身 logo。若严格要求 app executable 图标，应通过关联程序取其 icon，而非视频文件 icon；须比较实际显示 |
| hover 一秒普通/图片/视频/exe/文件夹详细属性，长名首行 | 已实现。`src/ui_extras.rs:79` NameTip，额外500ms叠加GPUI500ms；`src/file_hints.rs:138` collect，`:186` image dims、`:208` duration、`:212` exe属性；`:95` 文件夹最多4096项/160ms、文件/子目录各2条。名称总在首行 | 一秒时先基础信息、helper 返回后补丰富信息；不支持的 provider 不会伪造属性；网络/云/链接为快响应跳过内容统计 |

## 结论

此范围内没有仅凭静态审计就能确认的高严重度功能断线；四行文件名、软换行重命名、链接内部导航的实现路径与 0.6.1 声明相符。仍不能宣布“所有需求全部验收通过”。最需要父任务 GUI 检查的是重命名 overlay、Details 滚轮轴、框选命中、三态颜色、视频右下角图标与分组标题选中色。

## 本轮后续修复（源码已更新，尚未发布）

父任务已补：视频右下角取关联 app executable 图标、分组标题持久选择及选择+hover 颜色、四行名称增加 text_ellipsis、视图切换强制重建 rows，以及 End 键远距离滚动锚定。上述表格中对应的原缺口已经改动，最终状态应以新打包版本 GUI 复测为准。

快速访问本轮发现原先 pin/unpin 只是本程序 settings overlay，已补 Windows 原生 canonical verb 后台 helper。只对明确点击的目标调用实际 Shell 菜单；文件不受系统 pin-to-home 支持、或系统操作失败时保留本地 overlay，并显示“未写入系统列表”或具体错误，不能假称系统成功。成功动作后刷新 namespace。系统成功时不留下本地 favorites/hidden 强制遮蔽，以免阻断系统侧后续更新。

测试还发现两项真实回归并修复：

- 快照逐项查询 context menu 加载 Shell 扩展，导致 >10 秒，UI 的 1600ms helper 被杀。已删除此查询，pin state 暂保留 None（不伪造全已固定）。发布前 debug 只读实测三次 10 项分别 583/628/592ms，再次复测 621/604/565ms；脚本 `.tools/verify_quick_access_readonly.py`，LOCALAPPDATA 在唯一 TEMP 中。
- 普通 filesystem 文件夹菜单在已固定时也可能暴露 pintohome，不能拿它判断取消固定。第一次自有 TEMP 测试 pin 成功但 unpin 被错误 noop，测试断言捕获；改为读取 Quick Access namespace item 的菜单后已成功移除并删除原自有测试目录。新唯一 TEMP 文件夹再做完整 pin→系统 namespace 出现→unpin→namespace 消失→删除自有目录全通过。脚本 `.tools/verify_quick_access_owned.py`。原有系统项未修改。

未知 pin 状态仍保留系统快速访问项目；显示弱化的图钉，菜单提供“从快速访问移除”，显式动作优先 unpinfromhome，其次 removefromhome（频繁访问项）。已知本地固定仍使用“取消固定”。不能将暂时未知解释为未固定，也不删除该项目。只读快照不再加载 context menu。

本轮 `cargo check` 完成；`cargo test --lib --no-default-features quick_access` 3 项通过。真实 pin/unpin 测试不覆盖故障 Shell 扩展，独立请求的 5 秒 deadline 由父/导航任务在 shell helper 中补入。
