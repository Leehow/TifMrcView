# R16 格式插件模板

[English](README.md)

这是一个 Apache-2.0 许可的最小 Rust 格式插件，面向 **TifMrcView 0.5.0 及以上版本**，
使用当前的插件 API **1** 和 WebAssembly ABI **1**。通过 `plugins/sdk` 实现
`Format::open`、包含元数据的图像信息，以及每个 Z 切片的原始像素读取配方。
不需要 npm 依赖。当前应用仍要求 `index.js`，用它把插件注册到插件窗口中。

## 复制并改名

在 TifMrcView 仓库内：

```sh
cp -R templates/format-plugin plugins/my-format
cd plugins/my-format
```

把 `plugin.json` 的 `id` 改为 `my-format`，与文件夹名一致。同步修改 `index.js`
中的 id、名称、版本和说明。提交市场前请使用唯一 id。可按需修改 Cargo 包名、
`wasm` 文件名和扩展名声明；`build.mjs` 会读取 Cargo 包名，把编译结果放到清单
指定的 `wasm` 路径。

创建**独立仓库**时，在 TifMrcView 仓库根目录执行：

```sh
cp -R templates/format-plugin ../my-format
mkdir -p ../my-format/vendor ../my-format/tools
cp -R plugins/sdk ../my-format/vendor/sdk
cp scripts/tmv-plugin.mjs scripts/plugin-manifest.mjs scripts/plugin-zip.mjs ../my-format/tools/
cd ../my-format
```

将 `Cargo.toml` 中的 SDK 依赖改为
`tifmrcview-plugin-sdk = { path = "vendor/sdk" }`，再按上述说明同步修改 id 和 JS 注册。
创建仓库时把 SDK 源码和 `tools/` 一起提交。附带的 `.github/workflows/build.yml`
适用于这个独立目录结构：执行测试、构建、校验并上传 zip 构件，不会自动发布市场。

也可以将路径依赖替换为 `Cargo.toml` 注释中的 **git 依赖**，把 `rev` 固定为
<https://github.com/Leehow/TifMrcView> 中经过审查的提交。Cargo 会在仓库中找到 SDK 包。

这种方式仍需复制 `tools/`。也可用 `TMV_PLUGIN_TOOLS` 指定包含这些公共脚本的绝对路径。
在主仓库中默认使用 `../../scripts`；`build.mjs` 不会自动下载依赖或安装 target。

## 修改与测试

在 `src/lib.rs` 实现格式解析，在 `plugin.json` 声明扩展名。通过 SDK 的 `file`
读取当前文件，用 `Recipe` / `Chunk` 描述像素位置，让应用在原生侧读取像素。
不要在 wasm 插件内使用 `std::fs`。`Info::metadata` 会显示在图像信息面板中；
本示例没有虚构物理像素尺寸。

R16 是本示例定义的无压缩格式：

| 字节偏移 | 内容 |
| --- | --- |
| 0 | 四字节魔数 `52 31 36 00`（`R16\0`） |
| 4 | 宽度：小端 u32 |
| 8 | 高度：小端 u32 |
| 12 | 深度：小端 u32 |
| 16 | `宽 × 高 × 深` 个小端 u16 像素 |

像素按行排列，X 最快，然后 Y、Z；只有一个灰度通道和一个时间点。
尺寸必须非零，每页不超过应用的 16 GiB 限制，深度不超过 2²⁴。
短文件头、错误魔数、算术溢出或不完整像素数据都会报错；末尾多余字节被忽略。

```sh
cargo test
cargo run --example make_sample -- sample.r16
```

生成的样本为 4 × 3 × 2（共 64 字节），展平后的第 `i` 个像素为 `i × 1000`，
`i = 0..23`。主机测试复用同一个样本生成器和解析器，检查元数据、页面配方、
像素值以及无效输入。主机侧 SDK 文件导入只是占位实现，因此这些测试不会执行 wasm。
应用侧安装、打开和解码测试可参考 TifMrcView 的 `src-tauri/tests/*_plugin.rs`。

## 构建与安装

准备 Node 22+、Rust、`wasm32-unknown-unknown` target，以及 PATH 中的 `unzip`
（zip 校验需要）。可在非计量网络上先运行一次
`rustup target add wasm32-unknown-unknown`。随后只需：

```sh
node build.mjs
```

脚本先校验源目录清单，再执行
`cargo build --target wasm32-unknown-unknown --release`，校验编译结果，
最终生成 `dist/<id>-<version>.zip`。zip 根目录包含 `plugin.json`、`index.js`，
以及清单 `wasm` 字段指定的模块，使用与市场打包器相同的可复现 zip 写入器。
Rust 源码、SDK、构建工具和样本不会进入安装包。

在应用中打开 **插件 → 加载插件…**，选择 zip，确认插件已启用，再打开
`sample.r16`。检查两个 Z 切片和图像信息面板中的 R16 元数据。

也可单独运行校验器：

```sh
# 在主仓库：
node scripts/tmv-plugin.mjs check templates/format-plugin
node scripts/tmv-plugin.mjs check templates/format-plugin/dist/format-plugin-1.0.0.zip
# 在独立插件仓库：
node tools/tmv-plugin.mjs check .
node tools/tmv-plugin.mjs check dist/my-format-1.0.0.zip
```

校验器与市场打包器共用清单规则，并额外检查应用版本兼容性（默认 0.5.0，
可通过 `--app-version X.Y.Z` 修改）、路径、内置格式冲突和 wasm ABI 签名。
源代码 crate 尚未构建 wasm 时只做清单校验并明确提示；zip 安装包不能缺少 wasm。
未知导入会产生警告，说明应用会拒绝该模块。校验器不会执行插件代码；
应用安装时才检查 `tmv_abi() == 1` 和运行时兼容性。静态检查通过不等于图像解码已验证。

## 提交到市场

向 **<https://github.com/Leehow/TifMrcView> 提交 PR**，将改名后的源码放入
`plugins/<id>/`，使用仓库内 SDK 路径，同步 JS 注册信息，保留测试，并在 PR
说明样本及预期结果。SDK 和格式接口说明见 `docs/format-plugins.md`。

维护者运行 `node scripts/pack-plugins.mjs <out-dir>`，构建 `plugins/` 下的 crate，
生成 `<out-dir>/plugins/<id>-<version>.zip`、最新版本别名 `<id>.zip` 和
`<out-dir>/plugins/plugins.json`。每个可下载的市场条目由清单加上 `file`
（带版本 zip 文件名）、`size`（zip 字节数）和 `sha256`（zip 摘要）组成。
不要手工编辑生成的索引。模板保留在 `templates/`，默认市场打包器不会发布它。
