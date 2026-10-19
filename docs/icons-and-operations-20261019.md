# 图标、刷新及文件操作本地验证（2026-10-19）

## 已实现

- 图标优先读取本机 Explorer SVG、系统 Segoe Fluent Icons / Segoe MDL2 Assets 字体和 Shell 图标资源；缺失时使用内嵌微软 Fluent UI System Icons SVG。官方原始资产、固定上游提交及 MIT 许可位于 resources/fluent-icons。移除手绘工具图标。
- 导航刷新使用系统字体 Refresh 字形 U+E72C。地址栏电脑图标与虚拟位置标签页使用同一 computer 图标。
- 任务栏窗口图标按 DPI 从本机 explorer.exe 图标组加载，HICON 生命周期跟随窗口；内嵌 EXE 回退图标为微软 Document Folder 彩色 SVG 的 PNG/ICO。
- 未按键的鼠标移动不进入 Explorer 实体更新；不存在原生图标映射时直接使用官方 SVG，避免无效请求和重试。
- 同标签页、目录和查询条件刷新时保留已有行与滚动位置；目录、查询或标签变化仍重置视口。
- 永久删除不再屏蔽系统进度和错误界面。进度标题接口不受支持（E_NOTIMPL）时继续删除。完成、取消或部分失败后重新读取列表；原生菜单删除也触发刷新。短操作的进度窗口由 Windows 决定是否显示。
- 新建文件夹原子创建“新建文件夹 1”，重名时递增，直接进入行内重命名，可通过既有撤销机制撤销。
- 亮色活跃标签页与导航面板使用同一白色；非活跃标签及顶栏使用较灰背景。
- 网络驱动器使用 Shell 修改显示名称；本地卷标只有遇到 E_ACCESSDENIED 才对单次重命名请求 UAC，不提权整个 GUI。
- 固定到开始单项使用 ShellExecuteExW 原生动词、INVOKEIDLIST 和同步生命周期；系统拒绝时明确说明限制，不修改权限。

## 已验证与限制

- 93 项核心测试（串行）通过；另 1 项交互式原生删除测试显式执行通过，分别验证自建临时目录的回收站删除和永久删除。默认自动测试跳过该交互测试。
- 桌面全部 target 编译检查通过。GUI 实测新建目录直接进入重命名，删除完成后目录消失且总数 151 变为 150；F5 后保留相同首个可见项目 file-005；亮色双标签配色通过。
- 静止状态三次 GPU Engine 采样均为 0%。尚未获得持续无按键鼠标移动的前后对照，不能承诺具体 GPU 降幅。
- 本机实测为 Windows 11；Windows 10 回退路径未在独立 Windows 10 主机实测。
- 未更改用户实际磁盘卷标、网络位置名称或开始菜单固定状态；UAC、网络重命名与具体 Start 策略效果需相应环境复测。Start 拒绝访问不能保证仅靠本次调用方式消除。
- 并行测试曾触发既有 Win32 IPC 时序测试的不稳定断言，串行全量通过；未修改 IPC 行为。
- 备份在 artifacts/before-icon-performance-*。保留之前人工测试 ZIP，不上传 GitHub，不发版。

## 参考与证据

- Microsoft Fluent UI System Icons：https://github.com/microsoft/fluentui-system-icons ，固定提交 a347af95478f989038cccbf085978865264c048a。
- 用户 ExplorerIconDemo.zip（SHA256 98063541374e286a09620d234767fe5877f69038b28c44f85653a35867a7dd00）仅阅读源码及库存，未执行 EXE。REA 库存证据 ev_cefd42f984565b6cb4c79866ea645940c5ca9a239121c400354401bcdb78c577。参考其系统资源加载和 Segoe 字形思路，未复制 C# 实现。
- REA 静态查看本机 explorer.exe 资源：ev_5252b6cbf1d2827c49b193e4aa298269a13944fe179b1b9727614e3b89e7333a；首个图标组含 16、20、24、32、40、48、64、256 像素。仅证明资源库存，不等于动态调用观察。
- 实际本机 windows.refresh.svg 为主题色路径；当前导航使用示例所示系统 U+E72C 字形，没有宣称反编译 Explorer 内部按钮代码。
- ShellExecuteExW 参数：https://learn.microsoft.com/en-us/windows/win32/api/shellapi/ns-shellapi-shellexecuteinfow 。
