# Glulx 实现验收记录

[English](glulx-validation.md) | [中文](glulx-validation.ZH.md)

更新：2026-09-10。当前实现状态 commit 为 `ebab7cb`；本地验收环境为 Linux x86_64。本文件描述当前代码，不宣称穷尽 Glulx、Glk、媒体或跨平台符合性。

## 当前验证

```sh
RUSTC_WRAPPER= cargo fmt --all -- --check
RUSTC_WRAPPER= cargo test --all-targets
RUSTC_WRAPPER= cargo clippy --all-targets -- -D warnings
RUSTC_WRAPPER= cargo build --release
```

当前检查通过：

- 271 个库测试通过；6 个手动性能测试保持 ignored。
- 6 个 CLI 测试通过。
- 格式检查、全目标 Clippy 和 release 构建通过。
- undo 回归证明：2 MiB 故事在 1 MiB undo 预算下可以保留单页快照；共享故事映像不计入该快照预算。
- 受限页回归证明：dirty pages 达到页预算上限时，会在构造完整候选页表前拒绝。
- 分发表包含全部 150 条官方 Glulx opcode 和 124 个官方 Glk selector。表完整不等于语义覆盖完整。

仓库提供可复现的微基准套件，覆盖指令分发、线性查找、文本输出、dirty-page 快照、文本布局和 CPU 画布栅格化：

```sh
RUSTC_WRAPPER= python3 tools/benchmark-engine.py --output "<output-dir>/glulx-engine-benchmark.json"
```

真实大故事可使用独立工具测量启动和 Linux 峰值 RSS/HWM，不会在输入故事旁写入文件：

```sh
python3 tools/benchmark-stories.py \
  "<story-dir>/story-a.gblorb" \
  "<story-dir>/story-b.gblorb" \
  --output "<output-dir>/glulx-real-story-baseline.json"
```

真实故事测量是启动/驻留内存 workload，不是完整通关。当前剩余的主要内存成本是故事容器、执行映像、VM 内存和媒体/资源数据；仓库目前没有使用 mmap 或 block compiler。

## 覆盖矩阵

主要本地证据见 [conformance.rs](../src/vm/conformance.rs)，实现限制汇总见[兼容性说明](compatibility.ZH.md)。

| 领域 | 当前覆盖 |
| --- | --- |
| VM 核心 | 文件头/checksum/布局校验、寻址、栈/局部变量、调用、字符串、搜索、整数和浮点指令、heap、加速、verify、restart、protect 及类型化失败。 |
| 可移植存档 | IFZS CMem/UMem、Stks、MAll、身份校验、损坏输入拒绝、重复注释/扩展，以及 Glulxe/Adventure 双向互操作样本。 |
| Undo 与会话 | 256 字节 dirty-page 快照、未变页共享、restart/protection 恢复、按 payload 计费的预算、旧会话迁移和非法页拒绝。 |
| Glk | 窗口树、流、文件、内存/资源流、Unicode、输入请求、计时器、超链接、样式、日期、图像、声音声道及 124 个 selector 分发。 |
| 资源 | 内嵌和外置无执行文件 Blorb、散装资源目录、IFhd 身份、发现优先级、元数据、封面、RDes、会话保留及资源缓存清理。 |
| 媒体 | PNG/JPEG、AIFF/OGG/MP3、MOD/XM/S3M/IT、可选 SONG，图像缩放/裁剪、流式重采样、重复/偏移和软件采样帧同步。 |
| 终端 | 交互 TTY 网格/状态、预填编辑、定时取消、即时字符输入、回显/终止键、文件提示、多窗口选择和终端恢复。管道模式仍是独立的文本/文件自动化协议。 |
| GUI | Linux Xvfb/软件 OpenGL 覆盖文本布局、CJK 回退、样式、网格编辑、图像绕排、超链接、媒体完成、图形缩放和会话恢复。 |

矩阵说明已覆盖的行为和回归入口，不表示所有合法/非法组合都已穷举。

## 参考样本

参考实现版本为 Glulxe `56ab8743bab565de307bd892c555d8d8897ed517` 和 CheapGlk `14d8aaf6e4150669762bd4646a5368e75c1eeee6`。样本来自官方 Glulx 样本页或 IF Archive，不随仓库分发。通过 `--fixtures` 传入样本位置，命令见上方及[规范清单](glulx-spec-checklist.ZH.md)。

维护的样本覆盖 Glulxercise、Unicode、资源流、Adventure、Sensory Jam、输入扩展、日期时间和多窗口启动。精确 hash 和参考命令选项应保存在测试产物中，不应写入机器专属的仓库路径。

## 平台状态

| 场景 | Linux x86_64 | Windows | macOS |
| --- | --- | --- | --- |
| VM、CLI、测试、Clippy、release 构建 | 已完成 | CI/构建覆盖；实机行为待验收 | CI/构建覆盖；实机行为待验收 |
| GUI、字体、DPI、原生对话框 | 本地 Linux 验收完成 | 实机验收待完成 | 实机验收待完成 |
| TTY 和管道宿主 | 本地完成 | 控制台验收待完成 | TTY 验收待完成 |
| 实体音频输出 | 已有软件采样帧证据；实体波形待验收 | 待验收 | 待验收 |
| 长篇游戏完整流程 | 待完成 | 待完成 | 待完成 |

新增验收结果应记录平台、构建 commit、工具链、输入路线和失败行为，然后再更新此表。

## 复现工具

合成媒体、样式、图形、资源、终端、SONG 和 codec 检查使用仓库脚本。生成的 fixture 和输出应写入临时目录或显式指定的输出目录：

```sh
python3 tools/make-media-fixture.py "<output-dir>/glulx-media.gblorb"
python3 tools/make-style-fixture.py "<output-dir>/glulx-styles.ulx"
python3 tools/check-graphics-ui.py --candidate target/debug/glulx-rs --output "<output-dir>/glulx-graphics-ui"
python3 tools/check-terminal.py --candidate target/debug/glulx-rs
```

参考检查需要单独提供样本和解释器路径，不得依赖开发者机器的专属绝对路径。
