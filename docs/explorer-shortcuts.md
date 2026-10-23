# 资源管理器快捷键对照

依据 [Microsoft Support：Keyboard shortcuts in Windows，File Explorer shortcuts](https://support.microsoft.com/en-us/accessibility/windows/keyboard-shortcuts-in-windows#bkmk_fileexp)。核对日期：2026-10-16。以下是中文功能归纳，不是官方文档逐字转载。

“已接入”表示当前源代码具有对应动作及按键绑定，不代表全部键盘、输入法、Shell 扩展场景都经过逐项人工验收。编辑文件名、地址、搜索文字时，输入框自己的快捷键优先；文件操作对话框和选项窗口阻止主文件列表响应。

## 已接入的文件管理按键

| 按键 | 本程序行为 |
| --- | --- |
| Alt+← / Backspace | 后退 |
| Alt+→ / Alt+↑ | 前进 / 上一级 |
| Alt+D / Ctrl+L / F4 | 地址输入 |
| Ctrl+E / Ctrl+F / F3 | 搜索输入 |
| F5 / Ctrl+R | 刷新 |
| Enter | 打开选中项目 |
| Alt+Enter | 属性 |
| Shift+F10 | 右键菜单 |
| Shift+右键项目 | Windows 完整右键菜单 |
| Ctrl+C / Ctrl+X / Ctrl+V | 复制 / 剪切 / 粘贴 |
| Delete / Ctrl+D | 确认后删除至回收站 |
| Shift+Delete | 红色独立确认后永久删除，不进回收站 |
| F2 / Ctrl+Shift+N | 重命名 / 新建文件夹 |
| Ctrl+A | 全选 |
| 方向键 | 移动选择 |
| Shift+方向键 | 范围选择 |
| Ctrl+方向键，Ctrl+Space | 移动焦点，切换该项选择 |
| Home / End | 首项 / 末项 |
| PageUp / PageDown | 按当前可见高度移动 |
| Shift+Home/End/PageUp/PageDown | 扩展范围 |
| Alt+P / Alt+Shift+P | 预览 / 详细信息窗格 |
| Ctrl+T / Ctrl+N | 新标签 / 新窗口 |
| Ctrl+Tab / Ctrl+Shift+Tab | 下一个 / 上一个标签 |
| Ctrl+1…9 | 对应序号标签；不存在则忽略 |
| Ctrl+W / Ctrl+F4 | 关闭标签；末标签关闭窗口 |
| Ctrl+Shift+1…8 | 超大、大、中、小图标、列表、详细信息、平铺、内容 |
| F6 / Shift+F6 | 在文件区、地址栏、搜索框、导航树之间正反轮转 |
| F11 | 切换全屏 |
| Ctrl+Shift+E | 异步展开当前目录的导航树路径 |
| Escape | 退出当前编辑或关闭弹出层 |
| Ctrl+拖动 / Shift+拖动 / Alt+拖动 | 强制复制 / 强制移动 / 创建快捷方式，确认后执行 |
| Ctrl+双击项目 | 打开全部选中文件；选中文件夹各开一个新窗口（Ctrl+单击仍只切换选择） |
| Shift+双击文件夹 | 在设置启用时于新标签页中打开 |
| Ctrl+滚轮 | 在八级查看方式之间调整，独立于界面 DPI |

Ctrl+Shift+E 只加载当前路径的祖先，保持懒加载；不会递归扫描所有磁盘。导航树聚焦后支持上下、Home/End、左右展开收起或定位父子、Enter/Space、小键盘加减号和星号，以及菜单键/Shift+F10。星号只展开至多16个直接子项，快速访问仍保持一层。详细信息模式的左右方向键不作树节点展开操作。

## 特意保持的差异

按用户要求，Ctrl++ / Ctrl+- 缩放整个界面，Ctrl+0 复位。原版数字小键盘 Ctrl++ 的列宽自适应没有沿用，避免与界面缩放冲突。支持的查看方式有八种，不为 Ctrl+Shift+9 猜测额外模式。

Win+E、Win+方向键、Alt+Tab、Alt+F4 属于系统级行为。本程序没有接管 Win+E；默认也不会替换系统文件夹关联，只有用户在「文件夹选项 → 高级」确认风险并启用实验性默认文件浏览器后，才会写入当前用户的 Directory/Folder/Drive 打开命令，并可从同一选项还原。

## 撤销与系统行为边界

| 按键 / 手势 | 当前边界 |
| --- | --- |
| Ctrl+Z / Ctrl+Y | 0.6.2 已实现本程序记录的复制、移动、重命名、新建、回收站删除的撤销/重做。覆盖、合并、永久删除和不能安全记录的操作会清空历史并提示；详见 file-history-062.md。 |

0.6.2 补充 Windows 将 Shift+数字转换为标点后的 Ctrl+Shift+1…8 兼容绑定；按视图变化重建布局，修复切换列表后仍沿用旧网格的问题。Home/End 远距离定位按逻辑行索引滚动，避免未测量的虚拟行高度导致跳转失败。

默认拖放同卷移动、跨卷复制。所有拖放操作仍需确认。右键拖动提供复制、移动、创建快捷方式、取消，再经确认执行。

## 源码定位

- [src/ui.rs](../src/ui.rs)：`run` 中的 `KeyBinding`；`NavigateFiles`、`ActivateNumberedTab`、`ChangeNumberedView`；`Explorer::key_navigate`；动作处理及模态窗口拦截。
- [src/input.rs](../src/input.rs)：文本输入框自己的编辑按键。
- [src/ui_extras.rs](../src/ui_extras.rs)：查看方式菜单、导航树懒加载、拖放确认。
- [src/shell.rs](../src/shell.rs)：复制、移动、回收站删除和系统属性操作。

官方页面说明不同应用可以调整部分快捷键。本表明确列出本程序的差异和边界，避免把源代码已绑定误写成与 Explorer 全量一致。
