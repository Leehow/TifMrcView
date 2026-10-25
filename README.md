# TifMrcView

一个又小又快的 TIFF / MRC 图像查看器，基于 Rust + Tauri 2。双击 `.tif` / `.tiff` / `.mrc` 就能打开，拖动拉条翻 Z 层。

## 功能

- 支持格式：TIFF（多页、条带/分块、LZW/Deflate/PackBits/JPEG 压缩、8/16/32 位整数和浮点）、OME-TIFF（读取 C/Z/T 维度、通道名称和颜色、像素尺寸）、ImageJ hyperstack（包括 >4 GB 的连续存储文件）、蔡司 SEM TIFF（调色板灰度图，读取像素尺寸和全部采集参数）、MRC/MRC2014（mode 0/1/2/3/4/6/12/101，`.mrc` `.mrcs` `.rec` `.st` `.map`）
- 也能打开 PNG / JPEG / BMP / GIF / WebP
- Z 拉条和播放；有时间维度时另有 T 拉条
- 多通道彩色合成：勾选框显示/隐藏通道，点名字调这个通道的对比度，点圆点换颜色
- 亮度 / 对比度 / γ，直方图上拖动直接选显示范围；默认每层自动对比度（饱和 0.1%）
- 工具栏 ⓘ 按钮显示图像信息：格式、尺寸、像素尺寸，以及显微镜记录的采集参数（蔡司 SEM 的电压、工作距离、放大倍数、探测器、样品台位置、真空等，可搜索、可复制）
- 右侧预览栏列出同一文件夹里的图像（可收起），显示文件名、格式和大小
- 顶部图标工具栏（悬停显示名称）：存图、撤销、选择、移动、测距、画笔（再点一次切换 自由 / 框 / 圆）、裁剪、比例尺、亮度对比度
- 测距按像素尺寸显示真实长度；框和圆在图上一直标着宽 × 高（正圆标直径）
- 选择工具（默认）：拖空白处平移图像，点中标注后拖动、拖边框上的点缩放、拖测距线的端点重新测量，Delete 删除，可改颜色和线宽
- 标注按 Z 层保存，画完约 2 秒自动存到图片旁边的 `<文件名>.annot.json`，下次打开自动读回
- 裁剪：框出区域后双击或回车，只看这一块；Esc 或退格撤回
- 同名的 IMOD 模型（`.mod` / `.fid`）自动叠加显示，可按对象开关
- 存图：按原始分辨率存 PNG（裁剪后只存裁剪区域），带上显示中的标注、模型和比例尺
- 左边工具栏和右边预览栏拖内侧边缘可调宽度
- 鼠标滚轮缩放、拖动平移、双击适应窗口，状态栏显示像素坐标和原始值
- 拖文件进窗口即可打开；再双击同一文件夹的文件会在原窗口里打开，其他文件夹的文件会开一个新窗口
- 预览栏顶部的「打开文件夹」按钮会在访达 / 资源管理器里显示当前图像
- 用其他工具时在图上点右键，切回选择工具
- 标注可以只显示在一层，也可以跨一段 Z；选中后在上方点 Z 修改，Z 拉条上会标出有标注的层

## 插件

工具栏「插件」里可以开关插件、打开插件文件夹、检查更新。录制视频就是一个内置插件：在「工具」面板的「录制视频 (MP4)」里选 Z 或 T、起止帧、帧率、区域（整张/选区/当前视野）、尺寸和画质，录出来的 H.264 MP4 带有当前的对比度、通道颜色、标注和比例尺。

界面语言也是一个内置插件（`ui/plugins/i18n.js`）：支持中文、English、Deutsch、日本語、Español、Français、Русский，默认跟随系统语言，工具栏的下拉框可以手动切换。它会把界面上已知的中文文字换成所选语言，所以别的插件和提示信息也一起翻译；加新文字时在 `WORDS`（固定文字）或 `PATTERNS`（带数字、文件名的文字）里加一行。关掉这个插件就回到中文。

自己写插件：把一个 `.js` 文件放进插件文件夹，点「重新加载」即可。最小例子：

```js
TifMrcView.register({
  id: "hello",
  name: "示例插件",
  version: "1.0.0",
  description: "在工具面板里显示当前层",
  activate(app) {
    const box = app.ui.addToolSection("示例");
    app.on("slice", ({ z }) => (box.textContent = `当前第 ${z + 1} 层`));
  },
});
```

`app` 提供：`state()`（当前文件、Z/T、尺寸、选区、像素尺寸）、`on("open" | "slice" | "render", fn)`、`setZ` / `setT` / `open(path)`、`renderRegion(region, {z, t, size})`（按当前显示设置渲染成 canvas，含标注和比例尺）、`ui.addToolSection` / `ui.addToolbarButton` / `ui.addToolbarSelect` / `ui.toast` / `ui.busy`、`dialog.save`、`fs.write`、`settings.get/set`，以及调用 Rust 端的 `native(method, args, bytes)`。完整定义见 `ui/plugin-host.js`。需要原生代码的插件（如录制用的 H.264 编码器）在 `src-tauri/src/plugins/` 里实现 `NativePlugin`，并用 Cargo feature 控制是否编进去（`recorder` 默认开启，`--no-default-features` 可去掉，安装包小约 1 MB）。

插件和主程序有同样的权限，只安装自己信任的插件。

## 自动更新

应用启动几秒后会静默检查 `https://tifmrcview.com/tifmrcview/latest.json`，有新版本时顶部出现提示条，点「立即更新并重启」会下载、校验签名、安装并重启。更新包用 Tauri 的 minisign 密钥签名，公钥写在 `tauri.conf.json` 里。

## 快捷键

| 键 | 作用 |
| --- | --- |
| ↑ / ↓ | 上一个 / 下一个文件 |
| ← / →、PageUp / PageDown、`,` / `.` | 切换 Z |
| Home / End | 第一层 / 最后一层 |
| 空格 | 播放 / 暂停 Z |
| Ctrl/⌘ + 滚轮、Shift + 滚轮 | 切换 Z |
| S / V / M / P / R / E / C | 选择 / 移动 / 测距 / 画笔 / 画框 / 画圆 / 裁剪 |
| Shift（画框、画圆时） | 正方形 / 正圆 |
| Delete / 退格 | 删除选中的标注；没有选中时撤回裁剪 |
| Esc | 取消正在画的或刚画的标注、取消选中、撤回裁剪 |
| 回车、双击裁剪框 | 裁剪 |
| Ctrl/⌘ + Z | 撤销（画线、测距、移动、删除、裁剪都能撤） |
| Ctrl/⌘ + S | 存 PNG 到图片所在文件夹 |
| H | 显示 / 隐藏标注 |
| I | 显示 / 隐藏 IMOD 模型 |
| T | 打开 / 收起「工具」面板 |
| B | 显示 / 收起预览栏 |
| F、双击（移动工具） | 适应窗口 |
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

版本号自动生成：`tauri.conf.json` 里的主次版本（现在是 0.2）加上提交总数，例如 `0.2.57`；打 `v1.2.3` tag 时用 tag 的版本。

每次推送到 `main`，GitHub Actions 会在 macOS / Windows / Linux 上各打一次包，安装包在该次运行的 Artifacts 里。发布到下载站有两种方式：

- 打一个 `v` 开头的 tag：`git tag v0.3.0 && git push origin v0.3.0`，会上传到下载站并建 GitHub Release；
- 或在 Actions 里手动运行 build，勾选「发布到下载站」，用自动版本号发布。

发布时 CI 把安装包和更新包放到下载站的 `<版本>/` 目录，最后更新 `latest.json`（给自动更新用）和 `downloads.json`（给下载页用）。需要在仓库 Settings → Secrets and variables → Actions 里配置：

| 名称 | 类型 | 说明 |
| --- | --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | Secret | 更新包签名私钥（丢了就没法给已安装的用户推更新） |
| `DEPLOY_SSH_KEY` | Secret | 下载站部署用户的 SSH 私钥 |
| `DEPLOY_HOST` / `DEPLOY_USER` | Secret | 下载站地址和用户名 |
| `UPDATE_BASE_URL` | Variable，可选 | 默认 `https://tifmrcview.com/tifmrcview` |
| `DEPLOY_PATH` | Variable，可选 | 默认 `/var/www/tifmrcview` |
