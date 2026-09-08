# Glulx 解释器与 GUI 技术调研

[English](interpreter-research.md) | [中文](interpreter-research.ZH.md)

调研日期：2026-09-08。范围限定为项目自己的仓库、README、规范站点和 crates.io 注册信息；维护状态以最后提交/发布记录为准。

## 结论

线上已有成熟的 Glulx 解释器，但没有发现一个同时满足“活跃维护、纯 Rust、完整 Glulx、可嵌入、原生跨平台 GUI”的现成项目。

建议本项目实现自己的纯 Rust VM 和 Glk 抽象层，但不要从规范孤立开发：

1. 以 **Glulxe** 作为行为正确性的参考实现和差分测试 oracle。
2. 以 **Git** 参考解码、执行循环、缓存和加速策略，第一版不要照搬其动态编译复杂度。
3. 以 **Gargoyle** 参考“解释器通过 Glk 接入 GUI”的组件边界，而不是把 Gargoyle 源码直接嵌入 Rust 应用。
4. 以 **Quixe/GlkOte** 参考浏览器端窗口树、事件和存档交互；它适合 Web 目标，不适合作为原生 Rust VM 内核。
5. 把 **RemGlk / remglk-rs** 用作无界面测试、远程前端或 Web 服务适配层；它们不是 VM，也不是 GUI。
6. 不基于 `thefarwind/glulx-rs` 续写。该实现长期停更且明确未达到可运行 story file 的程度；选择性借鉴测试思路即可。

短期若目标是“尽快跑起真实游戏”，可增加一个可选的 C Glulxe 后端作为开发期基准；产品主线仍应保持纯 Rust VM。crates.io 的 `glulxe`/`glulxe-sys` 能完成这种桥接，但版本停留在 2019 年，不宜成为长期架构核心。

## 候选项目对比

| 项目 | 定位与语言 | GUI / I/O | 维护信号 | 许可证 | 对本项目的价值 |
| --- | --- | --- | --- | --- | --- |
| [Git](https://github.com/DavidKinder/Git) | C 编写的高速 Glulx 解释器 | 自身无 GUI，必须链接 Glk；仓库带 Windows Glk 构建支持 | 2026-08-23 仍有提交；当前仓库未归档 | MIT | 性能与执行器设计参考；适合差分测试，不适合直接形成纯 Rust 核心 |
| [Glulxe](https://github.com/erkyrath/glulxe) | C 编写的 Glulx 参考解释器 | 自身无 GUI，必须链接 Glk；可链接 RemGlk、CheapGlk 等 | 2026-05-05 仍有提交；README 已记录 3.1.3 双精度/undo 指令支持 | MIT | 最可信的兼容性基准；适合 oracle 或可选 FFI 后端 |
| [Quixe](https://github.com/erkyrath/quixe) | JavaScript Glulx VM，完全在浏览器运行 | 内含 GlkOte 浏览器 UI；支持文本/网格、输入、定时器、链接和实验性图形，但 README 明确仍不支持声音和 style hints | 2.2.6 发布于 2025-06-02；最后提交 2025-09-01 | MIT（附带构建工具另有 Apache-2.0） | Web 版完整参考和 UI 行为样本；不是 Rust/native 复用核心 |
| [RemGlk](https://github.com/erkyrath/remglk) | C 编写的 Glk RPC 实现，不是 VM | 没有 UI；stdout/stdin 交换 JSON，可与 GlkOte、bot、Web 服务或回归测试连接 | 0.3.2；最后提交 2025-06-12 | MIT；文档 CC BY-NC-SA 4.0 | 极适合自动化/远程前端协议参考，不可替代 GUI |
| [Gargoyle](https://github.com/garglk/garglk) | C/C++ 跨平台 IF player 与 Glk 实现，捆绑多种解释器 | Qt 5/6 或 macOS Cocoa；图像、声音、字体、滚屏、文件对话框等完整桌面体验 | 2026-09-07 仍有提交和多平台 CI | 主项目 README 声明 GPL；捆绑解释器各自保留许可证 | 最完整的桌面集成参考；依赖和许可证面较大，不宜直接移植进轻量 Rust GUI |
| [`glulxe` crate](https://crates.io/crates/glulxe) / [`glulxe-sys`](https://crates.io/crates/glulxe-sys) | Rust API + 内嵌 C Glulxe FFI | 调用方必须提供 Glk handlers；`main()` 会接管当前线程直到结束 | 最新 0.2.0，2019-11-04 发布 | 包装层 MIT OR Apache-2.0；内层 C/`-sys` MIT | 可做开发期 oracle/兼容后端；API、上游快照和维护状态偏旧 |
| [`glk` crate](https://crates.io/crates/glk) | 用 Rust traits 实现供 C 解释器调用的 Glk provider | 不自带 GUI；示例只有单窗口 terminal ToyGlk | 最新 0.2.0，2019-11-04 发布，文档指向 Glk 0.7.5 | MIT OR Apache-2.0 | 可参考 traits/FFI 映射，不宜直接锁定长期 UI API |
| [`thefarwind/glulx-rs`](https://github.com/thefarwind/glulx-rs) | 早期纯 Rust Glulx 实验 | 无 GUI/Glk 成品 | 最后 push 2018-02-10；仓库仅少量 VM/memory/stack 文件 | MIT | 不可作为基线；可阅读但不应 fork 续写 |
| [`curiousdannii/remglk-rs`](https://github.com/curiousdannii/remglk-rs) | 活跃的 Rust RemGlk/GlkOte 协议实现，不是 Glulx VM | 通过 `GlkSystem` trait 发送 update、接收 event；含 Blorb 与 C API 层 | 最后提交 2026-08-24；工作区版本 0.1.0 | MIT | Rust 侧最值得借鉴/复用的 Glk 协议部件；原生 GUI 仍需自己实现 |

## 逐项分析

### Git

Git 的目标就是速度。官方 README 声称它约为 Glulxe 的五倍，并通过可调缓存大小在速度和内存之间取舍；源树将 `compiler.c`、`peephole.c`、`opcodes.c`、`operands.c`、`memory.c`、`savefile.c` 和 `glkop.c` 分开，适合研究快速执行器的职责划分。[README](https://github.com/DavidKinder/Git/blob/master/README.md) [source tree](https://github.com/DavidKinder/Git)

它并不是 GUI 程序。README 明确要求链接一个 Glk library，仓库中只额外包含 Windows Glk 的构建材料。MIT 许可证允许借鉴或移植，但复制代码时仍须保留版权与许可通知。[LICENSE](https://github.com/DavidKinder/Git/blob/master/LICENSE)

建议：先实现直接解释器并用测试建立正确性，再按 profile 引入 decoded-block cache。不要在 MVP 就移植 Git 的代码生成器，因为它会显著放大 unsafe、平台差异和调试成本。

### Glulxe

Glulxe 的仓库自称 “The Glulx VM reference interpreter”。README 说明它必须与 Glk library 链接，并可使用 CheapGlk、GlkTerm、RemGlk 等后端；这正好验证了 VM 与显示层解耦是 Glulx 生态的既有边界。[README](https://github.com/erkyrath/glulxe/blob/master/README.md)

版本历史显示 0.6.0 已支持 Glulx 3.1.3 的 `hasundo`、`discardundo` 和双精度指令，后续主分支又增加 autosave/恢复能力；2026-05-05 的提交仍在处理扩展内存映射，说明它是活跃且覆盖边角行为的最佳 oracle。[latest commit](https://github.com/erkyrath/glulxe/commit/56ab8743bab565de307bd892c555d8d8897ed517) [LICENSE](https://github.com/erkyrath/glulxe/blob/master/LICENSE)

建议：测试中对同一 `.ulx` 输入序列同时运行 Rust VM 与 Glulxe/RemGlk，比较 JSON 输出、退出状态和保存文件。Glulxe 的 C 源只作为参考或隔离的 FFI feature，不能让 VM 核心依赖它。

### Quixe 与 GlkOte

Quixe 是纯 JavaScript Glulx VM，可在无服务端的浏览器中运行 `.ulx` 或 `.gblorb`。其 VM 核心、Glk dispatcher、story/Blorb loader 与 GlkOte UI 分开，是 Web 端模块划分的重要参照。[Quixe README](https://github.com/erkyrath/quixe/blob/master/README.txt)

不过，官方 README 仍明确列出声音和 style hints 未支持，因此不能把“能在浏览器玩大部分游戏”理解为完整 Glk 覆盖。若以后增加 Web/WASM 前端，可复用其交互模型或直接将 Rust VM 输出映射到 GlkOte；原生桌面 MVP 没有必要引入 JavaScript runtime。[Quixe LICENSE](https://github.com/erkyrath/quixe/blob/master/LICENSE) [GlkOte README](https://github.com/erkyrath/glkote/blob/master/README.txt)

### RemGlk 与 remglk-rs

RemGlk 是结构化 I/O 后端：解释器把窗口变化编码为 JSON 写到 stdout，再从 stdin 读取 JSON event。它支持多窗口和大多数 Glk I/O，但官方文档反复强调它“不提供用户界面”。[RemGlk README](https://github.com/erkyrath/remglk/blob/master/README.txt) [protocol documentation](https://github.com/erkyrath/remglk/blob/master/docs.html)

`remglk-rs` 是该思路的活跃 Rust 实现。其 `GlkSystem` trait 已抽象文件操作、GlkOte update/event、Unicode 和时间/目录服务，源树还包含 Blorb、协议对象、窗口/流/声道及 C API 层。[README](https://github.com/curiousdannii/remglk-rs/blob/master/README.md) [`GlkSystem`](https://github.com/curiousdannii/remglk-rs/blob/master/remglk/src/lib.rs) [Cargo manifest](https://github.com/curiousdannii/remglk-rs/blob/master/remglk/Cargo.toml)

建议：不要让 VM 直接依赖 RemGlk JSON。定义项目自己的小型 `GlkHost`/事件接口；RemGlk、GUI、headless recorder 都作为 adapter。可以从 git 依赖试用 `remglk-rs`，但在公开 API 尚未稳定且未见 crates.io 正式条目时，不要让核心类型泄漏到 VM API。

### Gargoyle

Gargoyle 是完整的跨平台 IF player，而不是单一 Glulx VM。官方 README 表明它同时打包 Git、Glulxe 等解释器；构建文件把每个解释器编译为独立 executable，并统一链接 `garglkmain` 和 `garglk`，这是一条重要设计证据：GUI/Glk 是平台层，Git/Glulxe 是可替换引擎。[README](https://github.com/garglk/garglk/blob/master/README.md) [interpreter build](https://github.com/garglk/garglk/blob/master/terps/CMakeLists.txt)

当前 GUI 在非 macOS 平台使用 Qt Widgets，在 macOS 可使用 Cocoa；声音可选 Qt、SDL2、SDL3 或关闭，并另行处理 JPEG/PNG、字体、TTS 等。完整复制会引入庞大 native 依赖和多许可证义务，但其窗口树、排版、scrollback、输入历史、全屏、主题和无障碍体验都值得做验收基准。[GUI build](https://github.com/garglk/garglk/blob/master/garglk/CMakeLists.txt)

本仓库是 `AGPL-3.0-only`。Git/Glulxe/remglk-rs 的 MIT 代码一般可以在保留通知后纳入，但 Gargoyle 自身及其捆绑组件的 GPL 版本差异需要逐文件审计；本节只是工程风险提示，不构成法律意见。[Gargoyle licensing statement](https://github.com/garglk/garglk/blob/master/README.md) [Gargoyle license inventory](https://github.com/garglk/garglk/tree/master/licenses)

### Rust 现成实现

crates.io 搜索目前主要返回两类条目：Glulxe C FFI (`glulxe`, `glulxe-sys`) 与 Glk provider (`glk`, `glk-sys`)，而不是完整纯 Rust VM。`glulxe` 的官方 crate README 也明确说它只是嵌入 C Glulxe，并且 `init()` 后由 `main()` 在同一线程接管执行；GUI 仍须提供所有 Glk handlers。[`glulxe` crate README](https://crates.io/crates/glulxe/0.2.0) [`glk` crate README](https://crates.io/crates/glk/0.2.0)

旧的 `thefarwind/glulx-rs` 在 2018 年停更。其源码包含多个 `unimplemented!()`，而 `glulxe` crate 的 README 也明确记录它在 2019 年底仍不足以运行 story files。因此不能把它当作能缩短交付时间的现成解释器。[repository](https://github.com/thefarwind/glulx-rs) [interpreter source](https://github.com/thefarwind/glulx-rs/blob/master/src/interpreter.rs)

crates.io 的关键词搜索还会返回并非 Glulx VM 的项目，例如 `glulx-asm`/`wasm2glulx`（生成 Glulx）和刚发布的 `rezrov`（其 workspace 当前只声明 `rezrov-zterp`，即 Z-machine）。这些不能满足本项目需求。[crates.io search](https://crates.io/search?q=glulx) [Rezrov manifest](https://github.com/jeffnyman/rezrov/blob/main/Cargo.toml)

## 对本仓库的具体架构建议

```text
glulx-core (纯 Rust，无 GUI/文件对话框)
  Story/Header -> Memory -> Decoder -> Executor -> Save/Undo
                                     |
                                     v
                               GlkHost trait
                              /      |       \
                    eframe adapter  headless  RemGlk adapter
                         |           recorder     (可选)
                    desktop GUI       |
                                   differential tests
                                      |
                             Glulxe + RemGlk oracle
```

边界应遵循以下规则：

- VM 不认识 eframe、Qt、JSON、窗口控件或操作系统文件对话框，只认识 Glk selector、参数和异步事件。
- GUI 维护 Glk window tree、文本/网格缓冲区、输入请求、图像/声音资源和菜单状态；VM 线程在等待 Glk event 时可暂停，不阻塞渲染线程。
- story/header/memory/operand/opcode 分层应以 Glulx 官方规范为准，并逐步覆盖 3.1.3 特性；官方规范入口是 [Glulx home/specification](https://eblong.com/zarf/glulx/) 和 [Glk home/specification](https://eblong.com/zarf/glk/)。
- MVP 优先支持：`.ulx`/`.gblorb` 加载、校验、整数执行、调用栈、字符串 I/O、核心 Glk 文本窗口与输入、quit/restart/save/restore/undo。之后再补浮点/双精度、heap、搜索、acceleration、图形、声音和完整 style hints。
- 从第一天保留 deterministic RNG、step limit、checked address arithmetic 与结构化错误；Glulxe/Git 都持续修复边界行为，真实 story file 不能被当作可信输入。

## 建议的复用清单

**直接复用**

- Glulx/Glk 规范作为唯一行为合同。
- Glulxe、Git、Quixe 的测试 story 和公开行为作为差分验证材料（逐项核对其资源许可证）。
- `remglk-rs` 的协议/Blorb 思路；是否作为依赖应在小型 spike 后决定。

**仅作参考或开发工具**

- Glulxe C / `glulxe` crate：oracle、兼容后端、故障定位。
- Git：性能优化和模块职责参考。
- Gargoyle：Glk 能力矩阵、桌面交互和发布矩阵参考。
- Quixe/GlkOte：未来 Web/WASM 前端参考。

**不采用为基础**

- `thefarwind/glulx-rs`：不完整且长期停更。
- 直接 fork Gargoyle：会把 Rust VM 项目变成 C/C++/Qt 集成项目。
- 把 RemGlk 当 GUI：它只定义结构化 I/O 通道。
- MVP 即实现 Git 风格动态编译：在兼容性测试成熟前收益低、风险高。

## 一手来源索引

- Glulx/Glk：[Glulx](https://eblong.com/zarf/glulx/)，[Glk](https://eblong.com/zarf/glk/)
- Git：[repository](https://github.com/DavidKinder/Git)，[README](https://github.com/DavidKinder/Git/blob/master/README.md)，[LICENSE](https://github.com/DavidKinder/Git/blob/master/LICENSE)
- Glulxe：[repository](https://github.com/erkyrath/glulxe)，[README](https://github.com/erkyrath/glulxe/blob/master/README.md)，[LICENSE](https://github.com/erkyrath/glulxe/blob/master/LICENSE)
- Quixe/GlkOte：[Quixe](https://github.com/erkyrath/quixe)，[GlkOte](https://github.com/erkyrath/glkote)
- RemGlk：[C implementation](https://github.com/erkyrath/remglk)，[Rust implementation](https://github.com/curiousdannii/remglk-rs)
- Gargoyle：[repository](https://github.com/garglk/garglk)，[README](https://github.com/garglk/garglk/blob/master/README.md)
- Rust registry：[glulxe](https://crates.io/crates/glulxe)，[glulxe-sys](https://crates.io/crates/glulxe-sys)，[glk](https://crates.io/crates/glk)，[glk-sys](https://crates.io/crates/glk-sys)
