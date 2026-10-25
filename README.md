# TifMrcView

一个又小又快的 TIFF / MRC 图像查看器，基于 Rust + Tauri 2。双击 `.tif` / `.tiff` / `.mrc` 就能打开，拖动拉条翻 Z 层。

## 功能

- 支持格式：TIFF（多页、条带/分块、LZW/Deflate/PackBits/JPEG 压缩、8/16/32 位整数和浮点）、OME-TIFF（读取 C/Z/T 维度、通道名称和颜色、像素尺寸）、ImageJ hyperstack（包括 >4 GB 的连续存储文件）、MRC/MRC2014（mode 0/1/2/3/4/6/12/101，`.mrc` `.mrcs` `.rec` `.st` `.map`）
- Z 拉条和播放；有时间维度时另有 T 拉条
- 多通道彩色合成：单击通道选中并调对比度，双击显示/隐藏，点圆点换颜色
- 自动对比度（每层计算，饱和 0.1%），也可以手动输入最小/最大值，带 γ 调节
- 「◀ ▶」按钮切换同一文件夹里的 tif/tiff/mrc 文件，底部是文件夹缩略图条
- 鼠标滚轮缩放、拖动平移、双击适应窗口，状态栏显示像素坐标和原始值
- 拖文件进窗口即可打开；已开着时再双击别的文件，会在同一个窗口里打开

## 快捷键

| 键 | 作用 |
| --- | --- |
| ← / → | 上一个 / 下一个文件 |
| ↑ / ↓、PageUp / PageDown、`.` / `,` | 切换 Z |
| Home / End | 第一层 / 最后一层 |
| 空格 | 播放 / 暂停 Z |
| Shift + 滚轮 | 切换 Z |
| F、双击 | 适应窗口 |
| 1 | 原始大小 (1:1) |
| Ctrl/⌘ + O | 打开文件 |

## 为什么快

- 文件用内存映射读取，一个页面里的多个条带/分块在所有 CPU 核上并行解压
- 打开文件后，后台按离当前层由近到远把其它 Z 层预先解码进内存（最多 3 GB），拖拉条时直接出图
- 只渲染屏幕上看得见的区域、只算到屏幕分辨率，再把 8 位 RGBA 以二进制传给界面，所以 4000×4000 的图缩放平移也不卡
- 缩略图在单独的线程池里生成，不和当前图抢 CPU
- 界面是纯 HTML/JS，没有前端框架；Release 版开启 LTO、strip，安装包约 3–8 MB（Linux AppImage 自带 WebKit 所以约 78 MB）

## 安装

到仓库的 Releases 页面下载：

- **macOS**：`TifMrcView_x.y.z_universal.dmg`（Intel 和 Apple 芯片通用），拖进「应用程序」。因为没有苹果开发者签名，第一次打开会被拦：到「系统设置 → 隐私与安全性」点「仍要打开」，或在终端执行 `xattr -dr com.apple.quarantine /Applications/TifMrcView.app`。首次启动时 TifMrcView 会把自己设为 .tif/.tiff/.mrc 的默认打开方式。
- **Windows**：`TifMrcView_x.y.z_x64-setup.exe`，双击安装（不需要管理员权限）。安装器会注册 .tif/.tiff/.mrc 关联；如果你以前给 .tif 手动选过别的程序（比如“照片”），Windows 不允许软件偷偷改，点 TifMrcView 里的「设为默认」会打开系统设置页，选一下就好。
- **Linux**：Debian/Ubuntu 用 `.deb`，Fedora 等用 `.rpm`，其它发行版用 `.AppImage`。首次启动时会通过 `xdg-mime` 设为默认打开方式。

## 从源码构建

需要 Rust（stable）和 Node.js 18+。Linux 还需要：

```sh
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libxdo-dev libssl-dev
```

然后：

```sh
npm install
npm run dev      # 开发模式
npm run build    # 生成当前系统的安装包，在 src-tauri/target/release/bundle/
```

调试解码可以用自带的小工具，它会打印元数据、各通道像素和以及解码/渲染耗时：

```sh
cd src-tauri && cargo run --release --example probe -- /path/to/file.tif
```

## 发布

推送到 `main` 时 GitHub Actions 会在 macOS / Windows / Linux 上各打一次包，安装包在该次运行的 Artifacts 里。打一个 `v` 开头的 tag 会自动建 Release 并附上所有安装包：

```sh
git tag v0.1.0 && git push origin v0.1.0
```

## 代码结构

```
src-tauri/src/
  lib.rs           Tauri 命令、文件打开/单实例/默认程序入口
  image.rs         像素平面、元数据、页面缓存和后台预取
  render.rs        按可见区域渲染、自动对比度、缩略图
  formats/tiff.rs  TIFF / OME-TIFF / ImageJ 读取（并行解压）
  formats/mrc.rs   MRC 读取
  assoc.rs         各系统设置默认打开方式
ui/                界面（index.html / app.js / style.css）
packaging/linux/   .desktop 模板、MRC 的 MIME 定义、deb/rpm 安装后脚本
```

## 许可证

本项目以 [Apache License 2.0](LICENSE) 开源。
