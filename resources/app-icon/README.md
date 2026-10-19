# 应用图标

窗口和任务栏优先从本机 `Windows/explorer.exe` 图标组读取相应 DPI 的图标，不复制或分发 Windows 图标资源。

EXE 内嵌回退图标采用微软 Fluent UI System Icons 的 Document Folder 彩色图标，将原始 SVG 渲染为多分辨率 PNG/ICO。不使用自绘或 AI 生成图标。

- 原始 SVG：`../fluent-icons/app-folder.svg`
- 上游版本及原始地址：`../fluent-icons/SOURCES.json` 中的 `app-folder`
- MIT 许可证：`../fluent-icons/LICENSE`
- 包含 16、20、24、32、40、48、64、96、128、192、256 像素。

这不改变本项目的名称或许可证，也不表示微软为本项目背书。
