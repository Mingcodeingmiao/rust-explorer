# 0.7.7 part1：交互、缩略图与搜索

本次包含四语言界面修改，以 0.7.7 part1 发布；内部基础版本号保持 0.7.7。Windows 原生右键菜单仍有已知问题，详见 [发布说明](v0.7.7-part1.md)。

## 行为

- 文件夹选项 → 细节：新增“双击空白区域返回上级文件夹”，默认关闭，旧设置文件也不会自动开启。
- 修复同一鼠标位置快速连续进入目录：GPUI 将连续点击累计为 3、4、5 次，原先只接受第 2 次。文件打开现在按 2、4、6…成对触发，分组折叠和文本编辑的点击规则不受影响。
- 标签页关闭按钮整体右移 4 个逻辑像素，保留居中的图标和点击区域。
- 搜索结果默认显示详细信息，含名称、文件夹路径、修改日期、类型与大小。搜索列宽、顺序独立保存；路径列可排序。清空搜索恢复普通目录查看方式。
- 当前搜索位置的文件系统只有确认是本地 NTFS 时才使用 Everything。非 NTFS、网络或无法确认的文件系统跳过 Everything，使用可取消的文件名遍历；不跟随目录重解析点。Everything 调用失败也可回退，不会直接留下空白错误页。文件系统回退按普通名称匹配，不解释 Everything 高级查询语法；结果上限 20,000 项。
- 修复原生空白/文件夹菜单的共享子菜单依赖 Explorer 视图上下文的问题：保留系统父标题，将两个动作明确映射到系统共享工作流。不编辑 ACL、不自动提权或提交共享；删除访问仍交给系统处理。

菜单保留窗口 owner、应用进程内独立 STA 以及既有消息泵，菜单本身没有改回一次性 helper。真正的共享窗口仍使用独立、可停止的系统操作生命周期。

## 统一 SQLite 图片缓存

在文件夹选项 → 高级 → 缩略图存储目录中设置本地绝对路径。留空使用 `%LOCALAPPDATA%/RustExplorer/cache/thumbnails`。应用设置并重启后生效。图标、缩略图以及 PDF/视频/字体预览共用同一个数据库；旧 PNG/JSON 文件缓存会在升级启动时自动清理，显式清空缓存也会清空 SQLite 记录。

数据库文件为 `thumbnails-v2.sqlite3`，使用内嵌 SQLite，无需安装数据库服务。数据库被删除后，下一次访问会自动创建新库并重建表结构；WAL/SHM 残留会被清理。

| 表/索引 | 用途 |
| --- | --- |
| `thumbnails`，`key` 主键，`WITHOUT ROWID` | 文件版本与尺寸到图像的映射；保存展示元数据、大小、访问时间、短期失败过期时间 |
| `icons`，`key` 主键，`WITHOUT ROWID` | 图标键到 BLOB 的映射，含大小和访问时间 |
| `previews`，`key` 主键，`WITHOUT ROWID` | PDF 页码/视频帧/字体预览键到 BLOB 的映射，含页数和访问时间 |
| `images`，`hash` 主键，`WITHOUT ROWID` | PNG BLOB，按 SHA-256 去重；不同项目相同图像只存一份 |
| `*_lru(last_access)` | 按最少使用顺序淘汰，避免遍历所有图像 |
| `*_image(image_hash)` | 索引图像引用，删除或替换记录后自动清理无引用 BLOB |
| `*_totals` 与触发器 | 常量时间维护记录数和逻辑字节数，避免每次写入求总和 |

键由路径、长度、修改时间、像素尺寸、链接版本和缓存版本组成并取 SHA-256。每线程复用连接；WAL、锁等待、NORMAL 同步、每连接约 4 MiB 页缓存。只在后台图标/缩略图线程和隔离的编解码进程中访问，GUI 绘制不执行 SQLite 查询。WAL 的并发与本地文件要求见 [SQLite 官方说明](https://www.sqlite.org/wal.html)。

缩略图最多 8,192 条 / 128 MiB，图标最多 8,192 条 / 64 MiB，预览最多 2,048 条 / 256 MiB；重复图像只存一份。数据库索引、空闲页和 WAL 另有开销。单幅 PNG 限制 16 MiB。命中时每分钟至多更新一次访问时间；后台写入以事务保证元数据和 BLOB 一致。真正“不支持缩略图”的结果缓存七天，磁盘、服务或编解码错误不写成长期“不支持”。

SQLite 直接保存 PNG BLOB，UI 通过 `Image::from_bytes` 解码；不再生成任何临时、按哈希命名的 PNG/JSON 文件。数据库不可写时仍可显示新生成的图像，不会阻止目录浏览。

## 验证边界

使用专用目录和独立设置文件测试，没有修改系统语言、默认文件夹关联或个人文件。

- 已交互验证单目录链 `a/b/c/d/e`、固定坐标连续四次点击进入两层、空白双击开关默认关闭及开启后的返回。
- 已验证存储目录对话框、保存、重启后在指定目录创建 SQLite 数据库，以及真实 PNG 的显示与 BLOB 落盘。
- 已验证原生共享子菜单展开、打开“特定用户”窗口并取消，未提交共享或执行删除访问。
- 非 NTFS 分支有文件名遍历及取消测试；本机没有为测试格式化设备，未声明 FAT/exFAT 设备实测。
- 菜单性能只记录本机所装扩展的实际范围，不把构建、源码或单元测试当作弹出延迟测量；系统 Shell 扩展的冷加载仍会影响首次菜单时间。

最终检查：141 项核心测试、1 项快捷方式集成测试通过，1 项既有交互删除测试跳过；全目标编译和格式检查通过。系统为 Windows 11 10.0.26300，浅色、应用缩放 100%。已验证跨进程重启后 SQLite 命中，搜索路径列调整为 380 逻辑像素而普通目录列宽保持原值，以及退出搜索恢复大图标。

原生菜单日志测量（本机安装了多项第三方扩展，开启诊断日志）：空白菜单第一次从菜单函数入口到准备调用显示约 644 ms，再次约 778 ms；其中系统 `QueryContextMenu` 阶段分别约 600/730 ms。此范围不含右键事件排队、实际绘制或用户停留时间，也没有禁用扩展的对照组，不能据此宣称通用菜单性能改善。文件菜单、此电脑菜单、取消，以及选择“新标签页打开”后的状态恢复已交互验证。

自动化检查入口：`scripts/check.ps1`。新增测试覆盖旧配置默认值、连续点击、后台名称搜索、SQLite 持久读取、失败过期、去重和孤立图像清理。

## Root sharing menu and Windows 10 menu themes

- Root directories offer Advanced Sharing; child directories retain Remove Access and Specific Users. No ACL or sharing changes were submitted during tests.
- Use the real Windows build to select the theme API: Windows 10 1809 uses the boolean ABI; 1903 and later use PreferredAppMode. Missing exports retain the system fallback.
- Reapply the theme after Shell extensions build their menus and before displaying them. Preserve the owner, in-process STA and cancellable file-operation boundaries.
- 140 core tests passed (one existing interactive test skipped), including root detection and theme ABI build boundaries. Windows 10 rendering still needs testing on that OS; third-party owner-drawn menus remain provider-controlled.
- References: [Microsoft PowerToys ZoomIt](https://github.com/microsoft/PowerToys/blob/main/src/modules/ZoomIt/ZoomIt/Utility.cpp), [win32-darkmode API definitions](https://github.com/ysc3839/win32-darkmode/blob/master/win32-darkmode/DarkMode.h).
