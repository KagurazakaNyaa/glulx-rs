# glulx-rs

[English](README.md) | [中文](README.ZH.md)

纯 Rust 实现的 Glulx 虚拟机，配有跨平台图形播放器。

虚拟机遵循 Glulx 规范，以 [David Kinder 的 Git](https://github.com/DavidKinder/Git) 作为行为参考。桌面操作流程参考 Windows Git：打开故事、在单个主窗口中游玩、查看历史输出、重启或停止虚拟机，以及配置字体和颜色。[Gargoyle](https://github.com/garglk/garglk) 用于参考跨平台 Glk 行为，而非视觉设计。

播放器实现了 Glulx 3.1.3 和 Glk 0.7.6，并明确限定可选能力的支持范围；目前还不能直接替代所有 Git 或 Glulxe 工作流程。具体实现范围见[兼容性说明](docs/compatibility.ZH.md)。

当前实现范围见[兼容性说明](docs/compatibility.ZH.md)，可复现证据见[验收记录](docs/glulx-validation.ZH.md)，规范边界和开放风险见[当前规范复核](docs/glulx-remaining-spec-audit.ZH.md)。

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

兼容性检查时可以用 `--strict-glk` 让未知 Glk selector 直接报告错误；默认模式
仍记录并忽略未知 selector，以兼容带有宿主扩展的故事：

```sh
cargo run --release -- --headless --strict-glk path/to/story.ulx
```

若需要记录实际交付给故事的事件，可在无界面模式中指定 JSON 输出路径：

```sh
cargo run --release -- --headless --trace-events "<output-dir>/events.json" path/to/story.ulx
```

该 trace 默认关闭，不进入存档或桌面会话。

桌面采用三个原生窗口：游戏画布、**日志＋输入**、**翻译**。调整或关闭辅助窗口不会改变画布尺寸；工具栏的 Log 和 Translation 按钮用于显示或隐藏窗口，Settings 打开独立设置窗口；也可从 View 菜单或快捷键 Ctrl+Shift+L / Ctrl+Shift+T 重新打开辅助窗口。画面在 Glk 事件边界发布，下一幅画面完成前保留上一幅完整画面。

游戏发起的保存/读取提示允许在输入栏填写路径；修改已有文件前需要确认，读取时要求文件已存在。桌面应用还会每 30 秒以及正常退出时保存会话；启动时不指定故事即可恢复上次会话。故事、内存和画布等快照原始数据超过 16 MiB 时，会跳过自动会话快照，避免界面卡死；这类游戏请使用游戏内的保存命令，播放器设置仍会保存。设置存储在可执行文件同目录的 `glulx-settings.json` 中，优先于旧 eframe 设置；首次没有 JSON 文件时迁移旧配置。点击 Save settings 或关闭设置窗口会立即保存，也会每 30 秒及正常退出时保存。手动编辑后重启生效；会话仍留在 eframe 存储中。`View -> Story information` 显示可用的 iFiction 元数据、封面以及图像/声音文字描述。可移植游戏存档与桌面会话使用不同的格式。

桌面支持段落对齐和缩进、带样式及原位编辑的网格、可缩放的行内和边缘图像、图片超链接，以及 MOD/XM/S3M/IT 音乐和采样音频。支持 Inform 加速函数 1–13。系统字体提供 Unicode 回退；Settings 中可调用 Windows/macOS 或 GTK 3 桌面的原生字体选择器，也可选择或填写 TTF/OTF/TTC 字体文件。所选字体优先用于对应的比例／等宽类别；等宽和 HW 字体用于网格及预格式化文本，在比例正文中作为缺字回退，不再覆盖比例拉丁字形。已测试的媒体格式及限制见兼容性说明。

启动前可指定外置资源：

```sh
cargo run --release -- --resources path/to/media.blorb path/to/story.ulx
cargo run --release -- --resources path/to/media-directory path/to/story.ulx
cargo run --release -- --no-auto-resources path/to/story.ulx
```

File → Choose resources 通过重启故事应用资源选择。交互终端中的 `--headless` 可显示网格、实时编辑行输入并接收单键；管道保留自动化协议。终端 Ctrl+N 切换待输入窗口，Ctrl+C 退出。已支持 SONG 音频和可用的真实 light 字体。

界面支持中文和英文。**设置 → 界面语言** 默认“跟随系统”，系统语言不受支持或无法检测时回退到英文。选择 **中文** 或 **English** 后立即生效，并保存到 `glulx-settings.json` 的 `language` 字段（`auto`、`en` 或 `zh`）。界面语言与故事翻译目标语言独立；系统原生对话框的内置控件遵循操作系统语言。 翻译文件和维护说明见 [assets/locales](assets/locales/README.md)。

## 性能与内存额度

请使用 release 或 diagnostic 构建测试游戏速度。解释器的指令操作数解码和搜索比较已去掉临时堆分配。
仓库提供固定微基准套件：500 万次分支指令分发、16,384 条记录的线性查找重复 1,000 次、4/8/16 KiB 文本输出、64 MiB 内存上的稀疏/密集 dirty-page 快照、16 KiB 冷/热文本布局和 512×384 CPU 画布栅格化。
基准工具会记录 commit、平台、Rust 版本、总耗时和每次操作耗时；这些指标不代表整款游戏的提速比例。
当前 Linux release 构建的真实故事启动 workload，对代表性的 650 MB 和 705 MB Blorb 文件测得峰值 HWM 约为 `661620 KiB` 和 `715360 KiB`。这只是启动/驻留内存测量，不是完整游戏基准；硬件、分配器和故事内容都会影响结果。

Settings 中每项额度均可选择**固定大小（MiB）**或**启动内存比例（1%–100%）**，保存在可执行文件旁的 `glulx-settings.json`。新配置的默认固定额度如下；已有配置中的数字仍按固定 MiB 保留：

| 设置 | 默认值 | 限制口径 |
| --- | ---: | --- |
| 游戏最大内存 | 1024 | VM 地址空间；含游戏堆，不含栈、原始故事副本或撤销快照；受 Glulx 格式限制，最多 4 GiB 减 256 字节 |
| 撤销快照 | 256 | 所有保留快照的页数据、栈字节和堆记录数据；共享故事映像不计入；不足时淘汰最旧快照，单份超限则保存撤销失败 |
| 图形图片缓存 | 512 | 缓存持有的 RGBA 像素及纹理像素数据；按最近使用顺序淘汰 |
| 正文图片缓存 | 256 | 正文图片缓存持有的纹理像素数据；按最近使用顺序淘汰 |
| 单张图片解码后大小 | 256 | 单张 RGBA 输出数据；超限资源不可用 |
| 单个音频资源原始大小 | 256 | 每个待播放资源及 SONG 引用资源的编码字节数；不是解码器总内存 |
| 每次 SONG 播放的采样数据 | 128 | 每次播放中去重后的 PCM 采样数据总量 |
| 进程硬上限 | 0 | 0 表示不添加限制；Windows 限制提交内存，Linux 限制虚拟地址空间 |

分类额度不等于进程总内存：已经显示的图片仍可由画布持有，驱动、解码器、容器和分配器也有额外开销。
缓存及撤销额度设为 0 时不保留相应缓存／快照；图片和音频额度设为 0 时相应资源不可用。
游戏、撤销和媒体额度在下次打开游戏或恢复会话时生效；进程硬上限在**重启程序后**生效。
降低游戏内存上限后，超过额度的存档／会话会被拒绝恢复，上限不会从存档中覆盖回来。

所有比例共用启动时探测一次的基数，运行期间不会随系统内存波动：

- Linux 有 cgroup v1/v2 限制时，取当前组及可见父级中最严格的有效内存**总上限**，不减去已用量。
- cgroup 未限制时，使用 `/proc/meminfo` 的 `MemAvailable`。
- Windows 使用 `GlobalMemoryStatusEx` 的空闲物理内存 `ullAvailPhys`。

设置窗口显示基数来源和换算额度。比例结果向下对齐：游戏内存按 256 字节对齐并受 32 位地址空间限制；资源额度按 MiB 对齐；进程额度按字节计算。
资源额度不再统一截断到 4095 MiB；SONG 不再额外受 1 MiB 文件大小限制，而是使用配置的音频资源额度。
探测失败时固定额度仍可使用，比例额度明确报错，不会静默退回一个过低的默认值。macOS 暂不支持比例探测。

**命令行 > 配置文件 > 默认值**，图形和终端模式一致。命令行仅覆盖本次运行，不会改写配置文件。
所有内存参数均接受固定 MiB 数字或百分比，例如：

```sh
cargo run --release -- --max-memory 25% --max-process-memory 75% --max-undo-memory 10% path/to/story.gblorb
cargo run --release -- --headless --max-memory 2048 --max-graphics-cache 8192 path/to/story.gblorb
```

其余参数为 `--max-text-image-cache`、`--max-decoded-image`、`--max-audio-resource` 和 `--max-song-pcm`。
配置文件支持混合使用数字和比例对象，例如：

```json
{
  "max_memory_mib": {"percent": 25},
  "max_process_memory_mib": {"percent": 75},
  "resource_limits": {
    "undo_mib": {"percent": 10},
    "graphics_cache_mib": 512
  }
}
```

Linux 使用 `RLIMIT_AS`，计入映射文件、共享库、线程栈预留等，不是 RSS 物理内存上限；
Windows 使用 Job Object 的进程提交内存限制。macOS 暂不支持进程硬上限，非零设置会明确报错。
无法安装系统限制时启动失败，不会静默忽略。额度过低可能导致启动或后续分配失败，
无法保证游戏能先保存再退出。若因此无法启动，使用 `--max-process-memory 0` 或修改 JSON 恢复。
已有更严格的系统限制仍然有效。

复现微基准并生成 JSON 证据：

```sh
python3 tools/benchmark-engine.py --output "<output-dir>/glulx-engine-benchmark.json"
```

工具内部使用 release、单线程和 `benchmark_` ignored tests；比较多次运行时应固定机器、电源模式、构建工具链和工作树 commit。

用同一真实故事和输入脚本比较 Glulxe、Git 与 Rust 的耗时、退出码和输出摘要：

```sh
python3 tools/benchmark-interpreters.py \
  "<story-dir>/story.gblorb" \
  --reference "<tool-dir>/glulxe" \
  --candidate "target/release/glulx-rs" \
  --git "<tool-dir>/git" \
  --command-file "<fixture-dir>/route.txt" \
  --repetitions 3 \
  --output "<output-dir>/glulx-interpreters.json"
```

输入脚本中的 `{save}` 会被替换为每个引擎独立的临时存档路径；benchmark 结果只
记录摘要，不把完整故事或输出写入仓库。

真实故事可用独立工具测量启动时间和 Linux 峰值 RSS/HWM；工具使用临时工作目录，不写入故事目录：

```sh
python3 tools/benchmark-stories.py \
  "<story-dir>/story-a.gblorb" \
  "<story-dir>/story-b.gblorb" \
  --output "<output-dir>/glulx-real-story-baseline.json"
```

## 翻译

回合翻译是可选功能，默认关闭。打开 `View -> Translation window`，在翻译窗口中启用，然后点击翻译窗口中的 **翻译设置…**，在独立窗口中配置兼容 OpenAI 的服务端点、模型、目标语言、提示词和采样参数。翻译设置与总体设置分开，两者关闭时都会保存配置。

通过兼容 Chat Completions 的 API，可配置 HY-MT2、DeepSeek、GPT 等模型的服务端点、模型标识、提示词和采样参数。请按所选模型支持的消息角色和参数填写；应用不提供模型专用预设，也不会自动覆盖配置。

系统提示词可以关闭或留空。用户模板支持 `{target}`（目标语言）和 `{text}`（原文）；不含 `{text}` 时在空行后追加原文，空模板直接发送原文。温度、top_p、top_k、重复惩罚和最大输出 token 数可独立勾选；未勾选时不发送该参数，沿用服务端默认值。`top_k` 和 `repetition_penalty` 需要服务端支持，输出限制使用 `max_tokens` 字段。旧配置在修改前保留 system 消息、原文 user 消息和 0.2 温度。缓存会区分提示词和采样参数。

播放器收集叙事输出，在虚拟机等待玩家输入时异步提交翻译。原文始终是权威内容，并立即显示。玩家命令不会被翻译。结果按原文顺序显示，同一会话、相同配置下的重复内容使用缓存，并合并相同的在途请求。开关只控制后续新输出，不补译旧段落、不清除已有译文，已提交的请求可在关闭后继续完成。当前原文和译文直接显示，旧界面内容置于默认折叠的历史记录中；同屏输出继续追加，游戏清屏后才切换当前内容。相同重绘不重复增加历史，提交前已清除的内容不再翻译。

默认模型名称为 `tencent/Hy-MT2-1.8B`；可以使用任何兼容的本地或远程端点。凭据保存在桌面应用的本地设置中，不会嵌入发布产物。

## 构建与测试

源码构建需要 Rust 1.95 或更新版本。CI 使用最新稳定 Rust；Cargo.toml 保留兼容的主／小版本范围，Cargo.lock 记录实际验证的精确版本。Runner 和 GitHub Action 的选择理由写在 [发布 workflow](.github/workflows/release.yml) 的对应注释中。

除常规图形界面构建依赖外，Linux 构建还需要 ALSA 开发头文件（Debian/Ubuntu 上为 `libasound2-dev`）。

```sh
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo build --release
```

可选的参考实现验证（测试样本需单独下载，见[验收记录](docs/glulx-validation.ZH.md)）：

```sh
python3 tools/check-reference.py \
  --reference /path/to/glulxe \
  --candidate target/debug/glulx-rs \
  --git /path/to/git \
  --strict-glk \
  --fixtures /path/to/fixtures
```

也可以把真实故事和输入脚本传给 `--route STORY COMMAND_FILE`；脚本中的 `{save}`
会替换为本次运行专用的临时存档路径。`--git` 和 `--route` 都接受真实路径，
不会依赖仓库外的固定文件。

可以在本地生成原创媒体样本，检查图像环绕、可点击图片、窗口缩放和 MOD 播放：

```sh
python3 tools/make-media-fixture.py "<output-dir>/glulx-media.gblorb"
cargo run -- "<output-dir>/glulx-media.gblorb"
```

段落样式和网格交互有单独的原创样本：

```sh
python3 tools/make-style-fixture.py "<output-dir>/glulx-styles.ulx"
cargo run -- "<output-dir>/glulx-styles.ulx"
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

硬件 OpenGL 环境下，画布图片由 GPU 缩放和混合；软件 OpenGL 使用缓存的 CPU 画布。诊断日志会记录渲染器和实际路径。
