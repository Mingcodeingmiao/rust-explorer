# 原生视频单帧与 PDF 预览

预览使用 Windows 系统 API。没有 WebView、Chromium、HTML 视频播放器，也不会创建持续播放或音频会话。阻塞的系统解析工作只能在 `--native-asset` 辅助进程中运行；界面从后台请求结果，获得 PNG 路径后绘制。

## 视频

`native_preview::video_thumbnail(path, size)` 通过 `SHCreateItemFromParsingName` 获取 `IShellItemImageFactory`：

1. 请求 `SIIGBF_THUMBNAILONLY | SIIGBF_INCACHEONLY`，优先使用 Explorer 的系统缩略图缓存。
2. 缓存没有时，再请求 `SIIGBF_THUMBNAILONLY`，让系统为该视频生成一张缩略图。
3. 失败时返回错误；不回退到文件图标来假装视频画面。

这与 Explorer 使用的已注册缩略图提供程序相容。实际可读的视频容器、编码、DRM 状态取决于系统安装的解码器及提供程序。系统决定缩略图的取帧时间，本程序不保证它恰好是第 0 帧。没有快速缩略图的文件不提供视频预览。

父进程给视频辅助进程 **1,500 ms** 的硬期限，包含启动、取帧及 PNG 编码。切换选择会取消旧请求；超时或取消时终止辅助进程。成功结果缓存到 `%LOCALAPPDATA%\RustExplorer\cache\native-v2`，缓存键包含路径、文件大小、修改时间和预览尺寸。

系统返回的 `HBITMAP` 用 `GetDIBits` 复制为顶部起始的 RGBA8 像素，视频帧按不透明处理。`HBITMAP`、内存 DC 和 COM 初始化均通过 RAII 释放。

官方行为及资源约定：[IShellItemImageFactory::GetImage](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-ishellitemimagefactory-getimage)。文档明确要求缩略图提取在后台进行，并要求调用方对返回的位图调用 `DeleteObject`。

## PDF

`native_preview::pdf_page(path, page, size)` 使用 `Windows.Data.Pdf.PdfDocument` 打开文件，通过 `PdfPage.RenderWithOptionsToStreamAsync` 渲染指定页，再用 Windows 系统图像解码器读取 RGBA 像素。`page` 从 **0** 开始，返回值同时包含文档页数，供界面翻页。

预览保留页面比例，输出最长边不超过请求尺寸（尺寸限制为 32–2,048 px）。本机 125% 桌面缩放会影响系统 PDF 渲染输出；像素解码阶段再次施加尺寸限制，使最终输出独立于桌面缩放。页面、内存流及 WinRT 初始化由 RAII 关闭。密码保护、损坏或系统不支持的 PDF 返回错误。父进程给 PDF 预览 **4 s** 硬期限。

官方接口：[PdfPage.RenderToStreamAsync](https://learn.microsoft.com/en-us/uwp/api/windows.data.pdf.pdfpage.rendertostreamasync?view=winrt-26100)。

## 本机实测（2026-10-16）

使用 GNU Rust 工具链、`windows 0.61.3` 和开发构建，直接调用阻塞 API，以 640 px 最长边测试。下列时间包含 COM/WinRT 初始化及像素读取，不包含父进程启动和最终 PNG 编码，也不代表所有文件的上限。

| 样本 | 输出 | 耗时 |
| --- | --- | --- |
| 现有 `flower.mp4` 首次请求 | 640 × 360 | 568 ms |
| 同视频复制到新路径后的首次请求 | 640 × 360 | 541 ms |
| 新路径的随后缓存请求 | 640 × 360 | 199 ms |
| 项目 `sample.pdf`，第 1 页 | 512 × 640，1 页 | 645 ms |
| 本机 Acrobat 中文模板 | 640 × 408，1 页 | 1,018 ms |
| 自建 2 页 PDF，第 1 页 | 512 × 640，2 页 | 691 ms |
| 自建 2 页 PDF，第 2 页 | 640 × 512，2 页 | 617 ms |

损坏的四字节 `.mp4` 和纯文本文件的缩略图调用均返回 `0x8004B200`，没有返回文件图标。对两页 PDF 请求索引 2 会返回“PDF 页码超出范围”。这验证了当前 MP4 样本、PDF 页面比例、页数和错误处理；未声称支持所有视频编码。

复测命令：

```powershell
cargo run --no-default-features --example native_preview_probe -- 'C:\path\video.mp4'
cargo run --no-default-features --example native_preview_probe -- 'C:\path\document.pdf' 1
```

探针直接执行阻塞 API，仅用于独立命令行验证；应用界面必须通过有限时、可终止的辅助进程调用。
