# 应用图标

恢复本项目原有生成图标，不使用微软图标作为应用标识。
原始 master 来自修改前的本地提交 1561865，内容未重绘；当前 master 在此基础上手工裁掉了透明边。
导出时不再裁切或补边，整幅 master 按目标尺寸等比缩放，画面构图与 master 保持 1:1。
PNG 由 `scripts/export-app-icon-png.py` 生成，ICO 再由 `scripts/export-app-icon.py` 打包。
包含 16、20、24、32、40、48、64、96、128、192、256 像素；窗口按系统 DPI 选择 EXE 内嵌图标。
工具栏缺失图标仍使用官方 Fluent SVG，与本项目应用图标分开处理。
