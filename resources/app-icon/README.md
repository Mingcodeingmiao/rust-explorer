# Rust Explorer 应用图标

原创设计：金黄色文件夹，融合深灰 Rust 齿轮与 R 标记，透明背景，Windows 11 风格。

- `rust-explorer-master.png`：imagegen 内置工具生成的原始图像，保留透明通道。
- `rust-explorer-{尺寸}.png`：16、20、24、32、40、48、64、96、128、192、256 像素。
- `rust-explorer.ico`：包含以上 11 种尺寸，由 Windows 选择合适的帧。
- `generation-prompt.txt`：本次生成使用的完整提示词。

图像尺寸导出使用 `scripts/export-app-icon.py`，采用 Lanczos 重采样。ICO 嵌入 EXE 资源 101；窗口初始化时按所在显示器 DPI 设置小图标和大图标，用于任务栏及 Alt+Tab。原始图像不要求随程序安装。

本目录是新生成的应用品牌图像，不是从 Windows DLL 或 EXE 中提取的系统图标。文件、文件夹和工具栏的系统图标加载逻辑不受影响。
