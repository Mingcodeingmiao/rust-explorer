# 后续计划 / Roadmap

本页记录方向，不是已实现功能清单，也不承诺具体日期。当前可用功能见 [0.7.5 更新说明](docs/v0.7.5.md)。

## 接下来

- 完善文件拖动/拖放交互，逐步验证跨窗口、外部程序和不同存储设备的行为及取消、冲突处理。
- 持续优化界面刷新、CPU/GPU 和内存使用、图标与缩略图加载以及大目录浏览。
- 优先修复可复现的 bug，继续验证高 DPI、窄窗口、亮暗主题和 Windows 10/11 差异。
- 继续改进默认文件夹打开与外部文件定位的可靠性，始终保留备份、限时试用和恢复原样的办法。
- 评估适配文件夹空白处双击返回上级等交互需求；双栏和顶部独立收藏栏也已记录，尚未实现。

## Issues 与贡献

- 已解决或已有替代设置的问题，会回复对应版本、入口及复测方法；部分完成的问题会明确剩余范围。
- 未实现的需求保留记录，后续逐步评估和适配。快速访问默认新标签页已在 0.7.3 提供；0.7.5 已将右键菜单改为进程内显示，剩余的类型列关联查询与图标路径缓存列入下方计划。
- 现有贡献 PR（包括鼠标侧键导航与依赖升级）本次只回复、不合并；之后逐项审核、解决冲突和回归测试，再逐步整合。没有承诺某个尚未审核 PR 一定合入。

## English

Future work focuses on more complete file drag/drop, performance improvements and reproducible bug fixes. Further folder-association reliability work will preserve recovery safeguards. Whitespace double-click navigation, dual panes and a separate favorites bar remain future evaluation items. Existing contributor PRs are deferred until individual review and regression testing; they are not part of 0.7.5.
