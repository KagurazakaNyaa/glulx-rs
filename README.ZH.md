# glulx-rs

[English](README.md) | [中文](README.ZH.md)

纯 Rust 实现的 Glulx 虚拟机，配有跨平台图形播放器。

虚拟机遵循 Glulx 规范，以 [David Kinder 的 Git](https://github.com/DavidKinder/Git) 作为行为参考。桌面操作流程参考 Windows Git：打开故事、在单个主窗口中游玩、查看历史输出、重启或停止虚拟机，以及配置字体和颜色。[Gargoyle](https://github.com/garglk/garglk) 用于参考跨平台 Glk 行为，而非视觉设计。

播放器实现了 Glulx 3.1.3 和 Glk 0.7.6，并明确限定可选能力的支持范围；目前还不能直接替代所有 Git 或 Glulxe 工作流程。具体实现范围见[兼容性说明](docs/compatibility.ZH.md)。

基于规范的[兼容性清单](docs/glulx-spec-checklist.ZH.md) 对照公开的 Glulx、Glk 和 Blorb 规范，跟踪已实现功能、剩余缺口和验证工作。

## 运行

```sh
cargo run --release
```

在播放器中打开 `.ulx`、`.blb`、`.blorb`、`.glb` 或 `.gblorb` 文件，也可以通过命令行传入故事文件，或将其拖到窗口上：

```sh
cargo run --release -- path/to/story.gblorb
```

自动化检查或没有显示服务器的系统可使用同一可执行文件内置的终端适配器：

```sh
cargo run --release -- --headless path/to/story.ulx
```

游戏发起的保存/读取提示允许在输入栏填写路径；修改已有文件前需要确认，读取时要求文件已存在。桌面应用还会每 30 秒以及正常退出时保存会话；启动时不指定故事即可恢复上次会话。`View -> Story information` 显示可用的 iFiction 元数据、封面以及图像/声音文字描述。可移植游戏存档与桌面会话使用不同的格式。

桌面支持段落对齐和缩进、带样式及原位编辑的网格、可缩放的行内和边缘图像、图片超链接，以及 MOD/XM/S3M/IT 音乐和采样音频。支持 Inform 加速函数 1–13。系统字体提供 Unicode 回退；可在 `View -> Options` 中选择额外字体。已测试的媒体格式及限制见兼容性说明。

## 翻译

回合翻译是可选功能，默认关闭。在 `View -> Translation panel` 中启用，然后在 `View -> Options` 中配置兼容 OpenAI 的服务端点、模型、目标语言和系统提示词。

播放器收集叙事输出，在虚拟机等待玩家输入时异步提交翻译。原文始终是权威内容，并立即显示。玩家命令不会被翻译。结果按回合排序，重复段落使用内存缓存。

默认模型名称为 `tencent/Hy-MT2-1.8B`；可以使用任何兼容的本地或远程端点。凭据保存在桌面应用的本地设置中，不会嵌入发布产物。

## 构建与测试

除常规图形界面构建依赖外，Linux 构建还需要 ALSA 开发头文件（Debian/Ubuntu 上为 `libasound2-dev`）。

```sh
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo build --release
```

可选的参考实现验证（测试样本需单独下载，见[验收记录](docs/glulx-validation.ZH.md)）：

```sh
python3 tools/check-reference.py --reference /path/to/glulxe --candidate target/debug/glulx-rs --fixtures /path/to/fixtures
```

可以在本地生成原创媒体样本，检查图像环绕、可点击图片、窗口缩放和 MOD 播放：

```sh
python3 tools/make-media-fixture.py /tmp/glulx-media.gblorb
cargo run -- /tmp/glulx-media.gblorb
```

段落样式和网格交互有单独的原创样本：

```sh
python3 tools/make-style-fixture.py /tmp/glulx-styles.ulx
cargo run -- /tmp/glulx-styles.ulx
```

Linux 桌面输入和图形回归工具使用隔离的 Xvfb 显示环境：

```sh
python3 tools/check-input-ui.py --candidate target/debug/glulx-rs
python3 tools/check-graphics-ui.py --candidate target/debug/glulx-rs
```

发布构建启用 LTO 并剥离符号。带标签的 GitHub 发布会构建 Linux、macOS 和 Windows 原生产物。Windows 产物为单个 `glulx-rs.exe`，内含虚拟机、GUI、TLS 客户端和翻译集成。

## 设计

核心接口刻意保持精简：`Story` 校验并提取执行映像，`Vm` 接收输入、产生文本并仅暴露运行状态，桌面播放器负责呈现和翻译。见[架构说明](docs/architecture.ZH.md)。

已有原生、浏览器、C 和 Rust 解释器的调研见[解释器技术调研](docs/interpreter-research.ZH.md)。

采用 AGPL-3.0-only 许可证。
