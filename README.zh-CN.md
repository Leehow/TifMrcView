[English](README.md) | **简体中文** | [Deutsch](README.de.md) | [日本語](README.ja.md) | [Español](README.es.md) | [Français](README.fr.md) | [Русский](README.ru.md)

# TifMrcView

一个小巧、快速、好用的显微镜与电镜图像查看器：支持 TIFF / OME-TIFF / ImageJ、冷冻电镜 / 冷冻电子断层的 MRC，并通过插件支持蔡司（CZI、LSM）、徕卡（LIF、LOF）、尼康（ND2）、奥林巴斯 / Evident（OIB、OIR）和 Imaris（IMS）的共聚焦格式、Gatan DigitalMicrograph（DM3、DM4）、赛默飞 / FEI（TIA SER、Velox EMD、EER）和 TVIPS 的电镜格式，以及蔡司 SmartSEM。可画线、框、圆并按物理单位测量，可切换 XY / XZ / YZ 视图，多通道荧光图可逐通道开关、换颜色。安装包只有几 MB，却能瞬间打开数 GB 的图像栈。支持 macOS、Windows 和 Linux，并可通过插件扩展。基于 Rust + Tauri 2。

[![Download](https://img.shields.io/badge/download-tifmrcview.com-2f6fed)](https://tifmrcview.com/tifmrcview/#download)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)
![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey)
[![GitHub stars](https://img.shields.io/github/stars/Leehow/TifMrcView?style=social)](https://github.com/Leehow/TifMrcView)

**[下载](https://tifmrcview.com/tifmrcview/#download)** · **[插件市场](https://tifmrcview.com/tifmrcview/plugins/)** · [官网](https://tifmrcview.com/tifmrcview/)

> 如果 TifMrcView 对你有帮助，请在 GitHub 上点个 ⭐，这对我们真的很有帮助。

## 截图

<table>
  <tr>
    <td width="50%"><a href="site/shots/mrc-tomogram-line.webp"><img src="site/shots/mrc-tomogram-line-1200.webp" alt="MRC 格式的冷冻电子断层图，带长度测量"></a><br><sub>MRC 格式的冷冻电子断层图，带长度测量和标注工具。</sub></td>
    <td width="50%"><a href="site/shots/mrc-xz-view-box.webp"><img src="site/shots/mrc-xz-view-box-1200.webp" alt="带框的 XZ 侧视图"></a><br><sub>XZ 侧视图，框上标出宽 × 高。</sub></td>
  </tr>
  <tr>
    <td width="50%"><a href="site/shots/czi-airyscan-measure.webp"><img src="site/shots/czi-airyscan-measure-1200.webp" alt="蔡司 CZI Airyscan Z 栈，带测量"></a><br><sub>蔡司 CZI Airyscan Z 栈，带测量和比例尺。</sub></td>
    <td width="50%"><a href="site/shots/tiff-multichannel.webp"><img src="site/shots/tiff-multichannel-1200.webp" alt="三通道荧光 TIFF"></a><br><sub>三通道荧光 TIFF，可逐通道开关。</sub></td>
  </tr>
</table>

## 功能

- 支持格式：TIFF（多页、条带/分块、LZW/Deflate/PackBits/JPEG 压缩、1–7 位灰度（如 1 位二值、4 位）以及 8/16/32 位整数和浮点；12 位等不常见位深会给出明确的错误提示）、OME-TIFF（读取 C/Z/T 维度、通道名称和颜色、像素尺寸）、ImageJ hyperstack（包括 >4 GB 的连续存储文件）、调色板 TIFF（如蔡司 SEM 图）、MRC/MRC2014（mode 0/1/2/3/4/6/12/101，`.mrc` `.mrcs` `.rec` `.st` `.map`；没有 IMOD 标记的 mode 0 文件会按文件头的最小/最大值判断是否为无符号字节），以及装上 Zeiss 插件后的蔡司 CZI（ZEN 保存的 LSM / Airyscan 等：不压缩、Zstd、JPEG 子块，8/16/32 位灰度和彩色，C/Z/T 维度、拼接大图、通道名称和颜色、像素尺寸）。同一个插件还能打开蔡司 LSM；其他格式插件每个厂商一个，可以打开徕卡 LIF / LOF（`lasx`）、尼康 ND2（`nis`）、奥林巴斯 OIB / OIR（`fluoview`）、Imaris IMS（`imaris`）、Gatan DM3 / DM4（`gatan`）、赛默飞 TIA SER、Velox EMD 和 EER 影片（`thermo`，还能读取赛默飞扫描电镜 / 双束电镜 TIFF 的参数和 MRC 的 FEI 扩展头）以及 TVIPS 影片（`tvips`，还能读取 EM-MENU TIFF 的参数）；一个文件里有多个图像时（如 LIF 工程、ND2 多位置），文件序号旁会出现图像选择框。`.rec` `.st` `.map` 和插件支持的格式只出现在「打开方式」里，不会被设为默认程序
- 也能打开 PNG / JPEG / BMP / GIF / WebP
- Z 拉条和播放；有时间维度时另有 T 拉条
- 体电镜（FIB-SEM、连续切片、SBF-SEM）：一个文件夹里的切片图像可以作为一个 Z 堆叠打开（把文件夹拖进窗口，或点文件夹按钮 / Ctrl/⌘+Shift+O）；切片可以是任何能打开的格式，用到哪张才读哪张。插件认得附近的采集项目文件时（装 `thermo` 插件后的赛默飞 Auto Slice & View `ProjectData.dat`），双击打开其中一张切片，缩略图列表最前面会多一个「堆叠」，Z 间距用项目里的切片厚度；没有项目文件时 Z 间距可以在「比例尺」里手动填
- 多通道彩色合成：勾选框显示/隐藏通道，点名字调这个通道的对比度，点圆点换颜色
- 亮度 / 对比度 / γ，直方图上拖动直接选显示范围；默认每层自动对比度（饱和 0.1%）
- 工具栏 ⓘ 按钮显示图像信息：格式、尺寸、像素尺寸，以及插件读出的采集参数（可搜索、可复制）。Zeiss 插件读取蔡司 SmartSEM 电镜图的电压、工作距离、放大倍数、探测器、样品台位置、真空等全部参数，并按校准像素尺寸显示比例尺，以及 CZI 里的显微镜、物镜、激光、探测器、针孔、扫描和 Airyscan 处理参数
- 右侧预览栏列出同一文件夹里的图像，显示文件名、格式和大小；左右两个侧栏顶部都有收起按钮，收起后同一行的窗口边缘会出现这个侧栏的图标（工具 / 预览图），点它展开
- 顶部图标工具栏（悬停显示名称）：存图、撤销、选择、移动、测距、画笔（点右边的小箭头选择 自由 / 直线 / 箭头 / 框 / 圆）、文字、裁剪、比例尺、亮度对比度
- 测距按像素尺寸显示真实长度；框和圆在图上一直标着宽 × 高（正圆标直径）
- 选择工具（默认）：拖空白处平移图像，点中标注后拖动、拖边框上的点缩放、拖测距线的端点重新测量，Delete 删除，可改颜色和线宽
- 标注按 Z 层保存，画完约 2 秒自动存到图片旁边的 `<文件名>.annot.json`，下次打开自动读回
- 裁剪：框出区域后双击或回车，只看这一块；Esc 或退格撤回
- 同名的 IMOD 模型（`.mod` / `.fid`）自动叠加显示，可按对象开关
- 存图：按原始分辨率存 PNG（裁剪后只存裁剪区域），带上显示中的标注、模型和比例尺
- 有 Z 的图可在顶部切换 XY / XZ / YZ 视图（侧视图里画的标注不保存）
- 左边工具栏和右边预览栏拖内侧边缘可调宽度
- 鼠标滚轮缩放、拖动平移、双击适应窗口，状态栏显示像素坐标和原始值
- 拖文件进窗口即可打开；再双击同一文件夹的文件会在原窗口里打开，其他文件夹的文件会开一个新窗口
- 预览栏顶部的「打开文件夹」按钮会在访达 / 资源管理器里显示当前图像
- 用其他工具时在图上点右键，切回选择工具
- 标注可以只显示在一层，也可以跨一段 Z；选中后在上方点 Z 修改，Z 拉条上会标出有标注的层

## 插件

工具栏最右边的齿轮（设置）里有「插件与更新」「界面语言」「帮助」。没打开图像时，窗口中间会列出直接支持的格式和每个插件能加上的格式，点「安装」即可装上插件。「插件与更新」里可以开关、安装和卸载插件，以及检查更新。除了多语言，插件都要单独安装：点「插件市场」在浏览器里打开 https://tifmrcview.com/tifmrcview/plugins/ 下载插件包（.zip），再点「加载插件…」选中它即可安装；装同一个插件的新版本会直接替换旧版本。插件源码在仓库的 `plugins/<id>/` 里（`plugin.json` + `index.js`，目前有 `recorder`、`zeiss`、`lasx`、`nis`、`fluoview`、`imaris`、`gatan`、`thermo`、`tvips`），装好的插件放在插件文件夹的 `<id>/` 子文件夹里。

一键安装（0.5 及以上）：在插件市场网页的插件卡片上点 **安装**，或者在「插件」窗口的「插件市场」列表里点 **安装**（有新版本时显示「更新到 x.y.z」）。网页会把 `tifmrcview://install-plugin?id=<id>`（可带 `&version=<x.y.z>`）这样的链接交给应用，应用先显示插件名称和说明，点 **安装** 后才会安装。它只安装官方插件市场（更新源旁边的 `plugins.json`）上列出的插件，通过 HTTPS 下载，并在安装前核对大小和 SHA-256。下载 zip 再点「加载插件…」的方式仍然可用。

格式插件：插件包还可以带一个在沙箱里运行的 WebAssembly 模块，用来支持新的文件格式或读取元数据，例如用 `plugins/sdk` 里的 Rust SDK 编写。每个厂商一个这样的插件（`zeiss` 打开 CZI、LSM 并读取 SmartSEM 参数），主程序里没有任何厂商格式的代码：没装 Zeiss 插件时打不开 .czi，应用会提示需要安装哪个插件。模块只能读取正在打开的文件，内存和运行时间都有上限；它把每一页描述成文件里的一组数据块，由应用本身并行解压、拼接。接口（ABI）、限制和示例见 [docs/format-plugins.md](docs/format-plugins.md)。

录制视频插件：在「工具」面板的「录制视频 (MP4)」里选 Z 或 T、起止帧、帧率、区域（整张/选区/当前视野）、尺寸和画质，录出来的 H.264 MP4 带有当前的对比度、通道颜色、标注和比例尺。

界面语言也是一个内置插件（`ui/plugins/i18n.js`）：支持中文、English、Deutsch、日本語、Español、Français、Русский，默认跟随系统语言，工具栏的下拉框可以手动切换。它会把界面上已知的中文文字换成所选语言，所以别的插件和提示信息也一起翻译；加新文字时在 `WORDS`（固定文字）或 `PATTERNS`（带数字、文件名的文字）里加一行。关掉这个插件就回到中文。

自己写插件：把一个 `.js` 文件放进插件文件夹，点「重新加载」即可；要分发的话，把 `plugin.json`（`id`、`name`、`version`、`description`、`main`、`apiVersion`、`minAppVersion`，可选 `author`、`i18n`、`details`，格式见 `plugins/recorder/plugin.json`）和脚本一起打成 zip（文件放在 zip 根目录）。最小例子：

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

`app` 提供：`state()`（当前文件、Z/T、尺寸、选区、像素尺寸）、`on("open" | "slice" | "render", fn)`、`setZ` / `setT` / `open(path)`、`renderRegion(region, {z, t, size})`（按当前显示设置渲染成 canvas，含标注和比例尺）、`ui.addToolSection` / `ui.addToolbarButton` / `ui.addToolbarSelect` / `ui.toast` / `ui.busy`、`dialog.save`、`fs.write`、`settings.get/set`，以及调用 Rust 端的 `native(method, args, bytes)`。完整定义见 `ui/plugin-host.js`。需要原生代码的插件（如录制用的 H.264 编码器）在 `src-tauri/src/plugins/` 里实现 `NativePlugin`；原生部分编进主程序，但只有装了同 id 的插件包才会启用。录制功能用 Cargo feature 控制是否编进去（`recorder` 默认开启，`--no-default-features` 可去掉，安装包小约 1 MB）。

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
| Ctrl + 滚轮（macOS 用 ⌘ + 滚轮）、Shift + 滚轮 | 切换 Z |
| S / V / M / P / L / A / R / E / X / C | 选择 / 移动 / 测距 / 画笔 / 直线 / 箭头 / 画框 / 画圆 / 文字 / 裁剪 |
| Shift（画框、画圆时） | 正方形 / 正圆 |
| Shift（画直线、箭头时） | 对齐 45° |
| Delete / 退格 | 删除选中的标注；没有选中时撤回裁剪 |
| Esc | 取消正在画的或刚画的标注、取消选中、撤回裁剪 |
| 回车、双击裁剪框 | 裁剪 |
| Ctrl/⌘ + Z | 撤销（画线、测距、移动、删除、裁剪都能撤） |
| Ctrl/⌘ + Shift + Z、Ctrl/⌘ + Y | 重做 |
| Ctrl/⌘ + S | 存 PNG（默认存到图片所在文件夹，也可在「工具」面板里指定保存文件夹） |
| H | 显示 / 隐藏标注 |
| I | 显示 / 隐藏 IMOD 模型 |
| T | 打开 / 收起「工具」面板 |
| B | 显示 / 收起预览栏 |
| F、双击（移动工具） | 适应窗口 |
| 1 | 原始大小 (1:1) |
| + / =、- | 放大 / 缩小 |
| Ctrl/⌘ + O | 打开文件 |

## 为什么快

- 文件用内存映射读取，一个页面里的多个条带/分块在所有 CPU 核上并行解压
- 打开文件后，后台按离当前层由近到远把其它 Z 层预先解码进内存（约占物理内存的四分之一，512 MB–8 GB），拖拉条时直接出图
- 只渲染屏幕上看得见的区域、只算到屏幕分辨率，再把 8 位 RGBA 以二进制传给界面，所以 4000×4000 的图缩放平移也不卡
- 缩略图在单独的线程池里生成，不和当前图抢 CPU
- 界面是纯 HTML/JS，没有前端框架；Release 版开启 LTO、strip，安装包约 4–11 MB（Linux AppImage 自带 WebKit 所以约 84 MB）

## 安装

到官网 **[tifmrcview.com/tifmrcview](https://tifmrcview.com/tifmrcview/#download)** 下载。版本更新频繁，只在官网发布（不再用 GitHub Releases），软件也从官网自动更新：

- **macOS**（10.15 及以上）：`TifMrcView_x.y.z_universal.dmg`（Intel 和 Apple 芯片通用），拖进「应用程序」。因为没有苹果开发者签名，第一次打开会被拦：到「系统设置 → 隐私与安全性」点「仍要打开」，或在终端执行 `xattr -dr com.apple.quarantine /Applications/TifMrcView.app`。首次启动时 TifMrcView 会把自己设为 .tif/.tiff/.mrc 的默认打开方式。
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

调试解码可以用自带的小工具，它会打印元数据、各通道像素以及解码/渲染耗时：

```sh
cd src-tauri && cargo run --release --example probe -- /path/to/file.tif
```

## 发布

版本号自动生成：`tauri.conf.json` 里的主次版本（现在是 0.3）加上提交总数，例如 `0.3.57`；打 `v1.2.3` tag 时用 tag 的版本。

每次推送到 `main`，GitHub Actions 会在 macOS / Windows / Linux 上各打一次包，安装包在该次运行的 Artifacts 里。发布到下载站有两种方式：

- 打一个 `v` 开头的 tag：`git tag v0.3.0 && git push origin v0.3.0`，会上传到下载站并建 GitHub Release；
- 或在 Actions 里手动运行 build，勾选「发布到下载站」，用自动版本号发布。

发布时 CI 把安装包和更新包放到下载站的 `<版本>/` 目录，最后更新 `latest.json`（给自动更新用）和 `downloads.json`（给下载页用）。下载页和插件市场（`site/`、`plugins/`）另有独立的工作流（`.github/workflows/site.yml`），`main` 上这些文件有改动时自动部署。需要在仓库 Settings → Secrets and variables → Actions 里配置：

| 名称 | 类型 | 说明 |
| --- | --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | Secret | 更新包签名私钥（丢了就没法给已安装的用户推更新） |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Secret，可选 | 该私钥的密码（如果设了的话） |
| `DEPLOY_SSH_KEY` | Secret | 下载站部署用户的 SSH 私钥 |
| `DEPLOY_HOST` / `DEPLOY_USER` | Secret | 下载站地址和用户名 |
| `UPDATE_BASE_URL` | Variable，可选 | 默认 `https://tifmrcview.com/tifmrcview` |
| `DEPLOY_PATH` | Variable，可选 | 默认 `/var/www/tifmrcview` |

## 代码结构

| 路径 | 内容 |
| --- | --- |
| `src-tauri/src/formats/` | 解码器：`tiff.rs`、`mrc.rs`、`raster.rs`（PNG/JPEG 等）、`mod.rs` |
| `src-tauri/src/` | `lib.rs`（命令、窗口和文件处理）、`render.rs`、`reslice.rs`（XZ/YZ 视图）、`image.rs`、`imod.rs`（IMOD 模型）、`mem.rs`、`assoc.rs`（默认程序注册）、`update.rs` |
| `src-tauri/src/plugins/` | 插件的原生部分（`recorder.rs`、`mp4.rs`）、插件包（`package.rs`）和 WebAssembly 格式插件宿主（`wasm.rs`、`recipe.rs`、`registry.rs`） |
| `src-tauri/examples/probe.rs` | 解码调试小工具 |
| `ui/` | 纯 HTML/JS 前端：`index.html`、`app.js`、`style.css`、`plugin-host.js`；内置语言插件在 `ui/plugins/i18n.js` |
| `plugins/<id>/` | 可安装插件的源码（`recorder`，以及编译成 WebAssembly 的 Rust crate 格式插件（每个厂商一个）`zeiss`、`lasx`、`nis`、`fluoview`、`imaris`、`gatan`、`thermo`、`tvips`）和插件 SDK（`sdk`） |
| `scripts/` | `pack-plugins.mjs`（打包插件 zip 并生成市场索引）、`make-release.mjs` |
| `site/` | 官网和插件市场，截图在 `site/shots/` |
| `packaging/linux/` | Linux 打包文件（.mrc 的 MIME 类型） |
| `.github/workflows/` | `build.yml`（构建和发布）、`site.yml`（官网和插件市场部署） |

## 翻译

README 提供多种语言（见顶部的语言栏）。**英文版 README（`README.md`）是基准**；其他文件的章节和顺序与它完全一致。修改 `README.md` 时请同步更新各语言版本，做不到的话请提个 issue。

## 许可证

本项目以 [Apache License 2.0](LICENSE) 开源。
