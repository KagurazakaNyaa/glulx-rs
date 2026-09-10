# glulx-rs、Glulxe 与 Git：基于当前实现的差异

复核日期：2026-09-10。

本文把两个成熟实现作为不同方向的基准：

- **Glulxe**：Glulx 参考解释器，用于行为、边界和可移植存档互操作。
- **Git**：David Kinder 的高速 Glulx 解释器，用于执行器、缓存和内存策略。

两者本身都不是完整桌面播放器，通常需要链接 Glk 实现。因而本文把
`glulx-rs` 的 VM、Glk 和播放器拆开比较；不把裸 Glulxe 或裸 Git 与 eframe
画布直接做视觉或交互比较。Gargoyle 是可以承载多个解释器的桌面 Glk 宿主，
只在宿主边界处作为补充参照，而不是第三个 VM 基准。

## 比较基线

| 对象 | 固定基线 | 角色 |
| --- | --- | --- |
| glulx-rs | 当前仓库 [`364f9cc`](https://github.com/KagurazakaNyaa/glulx-rs/commit/364f9cc2fb75a4baeb293d54d1073825d44c170b)；最近一份代码实现为 `f9c855d` | Rust VM、Glk 状态、桌面播放器和 TTY |
| Glulxe | [`56ab8743`](https://github.com/erkyrath/glulxe/commit/56ab8743bab565de307bd892c555d8d8897ed517) | C 参考 VM 与 Glk ABI |
| Git | [`8f5604e`](https://github.com/DavidKinder/Git/commit/8f5604e10c6194f7d0a6222491eaeb236a70a874) | C 高速 VM 与 Glk ABI |

当前 `HEAD` 相对 `f9c855d` 只有文档清理，因此本文的实现判断以工作树中的
源码为准。外部项目均固定到上表提交；构建宏、Glk 后端、字体、DPI、音频
设备和操作系统会改变运行时结果。本文是源码和已有验证记录的比较，不是同一
机器、同一故事、同一宿主配置下的性能排名。

## 结论

1. **功能覆盖已经接近可用播放器，主要差距不再是 VM 骨架。** 当前代码围绕
   Glulx 3.1.3 目标覆盖核心指令、字符串、heap、搜索、浮点/双精度、
   Inform 加速、IFZS、undo，以及相当完整的 Glk 窗口、流、事件、图像和声音
   路径。库测试当前为 `273` 个通过、`6` 个手工性能测试忽略，CLI 测试为
   `6` 个通过；这仍不是所有合法故事和所有宿主组合的证明。
2. **与 Glulxe 的差异主要在边界合同和宿主组合，而不是已有 opcode 的数量。**
   Glulxe 的 `exec.c`、`serial.c` 和 `glkop.c` 是当前最合适的行为 oracle；仓库
   已有 `tools/check-reference.py`，可以比较输出、IFZS 恢复、加速、长压缩字符串
   和共享文件流，但真实游戏路线和跨平台运行仍需扩大。
3. **与 Git 的最大结构差异仍是执行器。** `glulx-rs` 是带 2048 项 ROM
   decoded cache 的逐条 Rust 解释器；Git 会把指令编译成代码块并使用
   peephole/cache 机制。当前没有 block compiler、native/JIT 或 Git 风格的
   热路径专门化；是否值得加入必须由真实故事 profile，而不是单条指令微基准决定。
4. **`glulx-rs` 的独特价值在集成和资源治理。** 它在一个 Rust 产品中同时拥有
   VM、Glk 状态、eframe GUI、crossterm TTY、图片/音频解码、会话恢复和可配置
   内存额度。Glulxe/Git 把这些责任交给外部 Glk/播放器，因此更容易替换宿主，
   但不能单独提供同等的桌面体验。
5. **当前最实际的风险是“能启动”到“长期兼容”的距离。** 未覆盖的重点包括
   完整游戏路线、Windows/macOS 实机、所有 Glk 可选模块、历史 tracker 变体、
   字体和 DPI 差异，以及异常输入下与两个 C 实现的逐项行为差异。

## 总体对照

| 关注点 | glulx-rs 当前实现 | Glulxe | Git |
| --- | --- | --- | --- |
| VM 执行 | `src/vm.rs` 中直接解码并执行；错误返回 `VmError` | 以参考实现为目标的 C 执行循环，VM 与 Glk 分开 | C 执行器配合代码块编译器和 peephole 优化 |
| 指令缓存 | 2048 项固定索引 decoded cache；只缓存 ROM，RAM 代码不缓存 | 以直接解释和边界清晰为主，适合做差分基线 | 按 Glulx 地址查找已编译块，缓存大小影响速度和内存 |
| VM 内存 | ROM 从共享 `StoryImage` 读取；RAM 使用相对 `RAMSTART` 的可写 `Vec`，地址访问有边界和写保护 | `memmap` 管理故事内存，栈单独分配；`SERIALIZE_CACHE_RAM` 只影响存档缓存 | `gInitMem` 保存初始映像，`gMem` 保存运行内存；独立端口还提供映像映射路径 |
| undo | 256 字节 dirty-page 差分，未变页通过 `Arc` 共享；最多 16 份并按 payload 预算淘汰 | `saveundo` 保存 memory、heap、stack 记录，默认链长度为 8 | 有页级 undo 指针表，并把 undo 与代码缓存等运行时预算分开管理 |
| 可移植存档 | 输出 `CMem`、`Stks`、`MAll`；校验完成后才替换 VM | `serial.c` 是现有 Glulxe 互操作基线 | `savefile.c`/`saveundo.c` 提供自己的存档和 undo 实现 |
| Glk 边界 | 在 VM 内直接分发 selector，并保存窗口/流/事件/fileref 状态 | `glkop.c` 转到外部 Glk provider | `glkop.c` 转到外部 Glk provider，并保留 C ABI 快速路径 |
| 桌面和终端 | 自带 eframe GUI、crossterm TTY、管道协议和原生设置 | 不提供统一桌面 GUI；由 CheapGlk、RemGlk 等宿主承担 | 不提供统一桌面 GUI；端口和链接的 Glk 决定体验 |
| 媒体 | Rust 图片、采样、MOD/XM/S3M/IT、SONG 和 rodio 路径；部分准备异步 | 媒体由所链接的 Glk/平台处理 | 媒体由所链接的 Glk/平台处理 |
| 内存治理 | VM、undo、图形、文本图片、解码图片、音频和进程可分别设额度 | 取决于端口和操作系统策略 | README 和端口提供缓存/undo 选项，但不是与本项目相同的多类资源政策 |
| 失败模型 | 非法内存、opcode、栈、存档和输入可返回带类型的错误；未知 Glk selector 记录并返回零 | 参考实现行为受 C 端口和 Glk provider 影响 | 行为受 C 端口、编译选项和 Glk provider 影响 |

外部实现的职责依据见 Glulxe 的 [README](https://github.com/erkyrath/glulxe/blob/56ab8743bab565de307bd892c555d8d8897ed517/README.md)、
[执行器](https://github.com/erkyrath/glulxe/blob/56ab8743bab565de307bd892c555d8d8897ed517/exec.c)、
[存档](https://github.com/erkyrath/glulxe/blob/56ab8743bab565de307bd892c555d8d8897ed517/serial.c)、
[undo](https://github.com/erkyrath/glulxe/blob/56ab8743bab565de307bd892c555d8d8897ed517/saveundo.c)
和 [Glk bridge](https://github.com/erkyrath/glulxe/blob/56ab8743bab565de307bd892c555d8d8897ed517/glkop.c)，
以及 Git 的 [README](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/README.txt)、
[执行器](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/terp.c)、
[代码块编译器](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/compiler.c)、
[undo](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/saveundo.c)
和 [Glk bridge](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/glkop.c)。

## 1. VM 执行器

### 当前实现

`Vm::step` 负责一条指令的取码、操作数加载、执行和结果写回；
`operand_count` 明确列出支持的 opcode，未知 opcode 返回 `UnsupportedOpcode`。
`fetch_decoded` 对 ROM 地址缓存 opcode、操作数模式和下一条 PC，对 RAM 地址不缓存，
以保留自修改代码语义。操作数最多八个，取码过程检查地址模式和内存边界。

播放器在 `app.rs` 中以每批最多 1024 条指令、约 8 ms 的时间片推进 VM，并在
呈现版本变化或等待输入时交还 UI。TTY 使用更大的批次以减少交互开销。这个边界
是产品响应性策略，不是 Glulx 语义的一部分。

### 与 Glulxe 的差异

Glulxe 更适合作为“同一故事、同一输入、同一 Glk 后端”的行为基线。当前仓库
已经对寻址、调用栈、字符串续体、浮点特殊值、双精度 word 顺序、搜索、加速和
异常存档做了专项测试，但仍然主要是合成故事和有限官方样本。两边在错误输入、
宿主能力查询、时间事件和音频完成通知上的差异，应以事件/输出记录逐项确认，不能
只看最终退出码。

当前执行器没有证据表明它在所有真实故事上已经成为瓶颈。已有 release 微基准
记录约 `8.3 ns/指令`，headless 启动 workload 也记录了 decoded-cache 命中率；
这些数字没有与 Git 在同一故事、同一编译选项、同一 Glk 后端下测量，因此不能
推出“比 Git 慢多少”。

### 与 Git 的差异

Git 的核心差异不是一个更大的 `match`，而是 `compiler.c`/`compiler.h` 维护的
代码块生命周期：从 Glulx 地址找到块，未命中时解码和编译，随后执行缓存中的
内部代码，并按需要进行缓存淘汰或压缩。`peephole.c` 还可在编译阶段改写局部
指令序列。`glulx-rs` 当前只做指令元数据缓存，避免了动态代码生成的 unsafe、
平台差异和失效协议，但也没有获得 Git 的块级 dispatch 优势。

后续若要靠近 Git，应先完成：

1. 用长篇真实路线分离 opcode dispatch、内存访问、Glk 调用、字符串和排版成本。
2. 设计包含 RAM 写入失效、自修改代码、`select`/输入边界和 debug trap 的块失效协议。
3. 以 Glulxe 差分测试保护行为，再决定采用 decoded block、peephole 还是其他局部优化。

## 2. 内存、heap 与 undo

### 当前实现

`Memory` 把共享故事映像作为 ROM 基线，只为 `RAMSTART..当前末尾` 保存可写字节；
`setmemsize`/`malloc` 受 256 字节对齐和 VM 上限约束。写入会标记 dirty page，
undo 快照只复制相对于故事初始 RAM 或扩展区零值的变化页，临近快照的相同页通过
`Arc` 复用。栈、heap block 表、PC、续体目的地和内存长度另行保存。

这已经使 undo 的实际数据成本接近页级实现，但数据结构仍是 Rust
`BTreeMap<u32, Arc<Vec<u8>>>`，不是连续页指针表。候选快照还需要建立页表后再
判断细节成本；非常大的 dirty set 仍应继续 profile。

### 与两个成熟 VM 的差异

- Glulxe 和 Git 都把可移植存档的 Glulx 合同与宿主窗口状态分开；当前实现也
  将 IFZS 与桌面会话分开。IFZS 不回滚 Glk、RNG、I/O system、字符串表和保护区
  定义，桌面会话才保存窗口、资源、输入、画布和音频进度。
- Git 的 undo 结构直接围绕页指针和可配置 undo 缓冲设计，并把代码缓存作为另一个
  可调运行时成本；当前实现以差分页、栈字节和 heap 记录计费，同时有图形、音频、
  图片和进程额度，治理更细但元数据结构更重。[Git README](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/README.txt)
  记录了这些缓存/undo 配置的语义。
- 当前故事加载已经用 `Arc` 共享容器和执行映像，减少了重复驻留；但 VM 的可写
  范围仍按 `end_mem - ram_start` 预留，尚未使用 mmap 或惰性页。Glulxe 的
  `memmap` 和 Git 的独立 Windows 端口都提供各自的映像/内存管理路径，不能把它们
  的端口行为直接等同于当前 Rust 的 `Vec`。[Glulxe memory](https://github.com/erkyrath/glulxe/blob/56ab8743bab565de307bd892c555d8d8897ed517/vm.c)
  [Git Windows port](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/git_windows.c)

因此，当前实现的内存优势是可控性和可解释的失败边界，不是绝对驻留量已经优于
成熟 C 端口。大故事启动的现有测量仍应视为驻留 workload 证据，而不是完整性能
或内存排名。

## 3. Glk、事件与宿主边界

### 当前实现

`Vm` 内部保存窗口树、流、fileref、请求表、事件 FIFO、定时器、样式提示和图形
命令。selector 在 `vm.rs` 直接分发，宿主通过 `WindowView`、`GraphicsRequest`、
`InputRequest` 和 `provide_*` 方法与 VM 交互。图形 GUI、TTY 和管道模式通过
`set_graphical_host`/`set_terminal_host` 报告能力差异。

优点是 Rust 会话可以直接序列化整数句柄和窗口状态，输入与正在运行的 VM 也由
单一 owner 管理；缺点是 Glk 状态和播放器假设集中在本项目的 VM API 中，尚未有
一个可替换的公开 `GlkHost` trait。未知 selector 会记录并返回零，不能把这种
宽容行为当成所有 Glk provider 的一致合同。

### 与 Glulxe/Git 的差异

Glulxe 和 Git 的 `glkop.c` 都把解释器调用转换为外部 Glk ABI；真正的窗口、文件、
声音和图像行为由 CheapGlk、RemGlk、Gargoyle 或其他 provider 提供。这个边界让同
一个 VM 可以接 terminal、RPC、桌面或测试宿主，也意味着比较时必须固定 provider。
Glulxe 官方 README 明确说明需要链接 Glk library，并列出 CheapGlk、GlkTerm 和
RemGlk 等选择；Git 也要求链接 Glk。[Glulxe README](https://github.com/erkyrath/glulxe/blob/56ab8743bab565de307bd892c555d8d8897ed517/README.md)
  [Git README](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/README.txt)

这会形成两类差异：

| 层次 | 当前状态 | 兼容性含义 |
| --- | --- | --- |
| 标准 selector | 已覆盖主要窗口、流、Unicode、输入、事件、样式、图像、声音和日期路径 | 可比较 VM 到 Glk 的调用结果，但仍需按 capability/参数验证 |
| 可选能力 | 终端关闭图像、声音、鼠标和超链接；图形字符输入不提供；部分网格布局提示被忽略 | 与 GUI provider 的差异是明确能力差异，不应算作 opcode 缺失 |
| 事件模型 | 请求表 + FIFO 事件 + `select`/`select_poll`；玩家输入由宿主注入 | 需要比较事件顺序、取消、定时器和完成通知，不能只比较文本 |
| 对象状态 | VM 中持有可序列化整数句柄和窗口内容 | 方便桌面会话恢复，但与 C provider 的指针注册表不是同一数据结构 |

Gargoyle 的定位正好说明这一点：它是跨平台 IF player，构建时把 Git、Glulxe 等
解释器作为可替换程序接入共同的 Glk/GUI 宿主，而不是另一个 Glulx VM。[Gargoyle
interpreter build](https://github.com/garglk/garglk/blob/9597add4091e5aaf6ebc31399b049158e12ca565/terps/CMakeLists.txt)

## 4. 已核实的兼容性差异

这里列出已经能从当前源码和固定基线直接确认的差异；它们不是“某个实现看起来
更成熟”的泛化判断。

### 整数 `div/mod` 溢出

当前 `0x13`/`0x14` 只拒绝除数为零，然后使用 Rust 的 `wrapping_div`/
`wrapping_rem`。对于 `0x80000000 / -1`，Glulxe 和 Git 的执行器都显式拒绝，
而当前 VM 会产生 wrapping 结果。这是实际的语义差异，应作为高优先级兼容性
修复，而不是留给宿主处理。[当前实现](../src/vm.rs)
[Glulxe 执行器](https://github.com/erkyrath/glulxe/blob/56ab8743bab565de307bd892c555d8d8897ed517/exec.c)
[Git 执行器](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/terp.c)

### 未知 Glk selector

当前 VM 对未知 selector 记录后返回 `0`，这使兼容性失败变得宽容但不明显。
两个 C bridge 找不到对应 prototype 时则进入 fatal error。两种策略都可以是
产品选择，但差分工具必须把“返回零”和“解释器失败”区分记录，不能把最终文本
相同当成完整兼容。[当前分发](../src/vm.rs)
[Glulxe bridge](https://github.com/erkyrath/glulxe/blob/56ab8743bab565de307bd892c555d8d8897ed517/glkop.c)
[Git bridge](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/glkop.c)

### Git 自身的已知限制

Git 的 README 明列短局部变量和直接 search key 的限制；当前 `glulx-rs` 的
局部变量和搜索路径接受 1、2、4 字节。这里不能把 Git 的限制倒推成 Glulx
规范要求，也不能因为一个故事能在 Git 中运行就认为所有边界都已被覆盖。Git
适合作为第二套 oracle，但每个失败都要标记为“Git 限制”还是“规范差异”。
[Git README](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/README.txt)
[当前局部变量与搜索](../src/vm.rs)

## 5. 播放器、媒体与并发

这是 `glulx-rs` 相对于两个裸 VM 最大的产品级扩展，也是最容易产生错误比较的部分。

- 故事读取、VM 建立、图片解码、采样/MOD/普通 SONG 准备通过带请求 ID 的 worker
  异步完成；VM、Glk、eframe 上下文和活动会话仍由一个 owner 驱动。
- GUI 使用 LayoutJob/Galley 和自有文本/网格布局；图形窗口保留 Fill/Image 命令，
  在硬件路径中交给 GPU，在软件路径或快照时进行 CPU 栅格化。
- 音频走 rodio/Symphonia 和 Rust tracker 解码器；SONG 解析 Blorb 中的 AIFF 样本
  引用，并保留重复、暂停、通知和会话恢复语义。
- 桌面会话是播放器快照，不是 IFZS。大型会话原始数据超过 16 MiB 时会跳过自动
  序列化，游戏内可移植存档仍可用。

Glulxe/Git 核心不负责上述统一 GUI、字体、图片缓存或音频设备；这些责任落在它们
链接的 Glk provider 和播放器。Gargoyle 因而适合拿来比较窗口树、FreeType 字体、
scrollback、文件对话框和多解释器发布体验，但不应拿来证明 Git VM 的指令语义。

当前播放器的主要取舍是：异步准备减少 UI 阻塞，单一 VM owner 保证事件和 undo
边界清晰；纹理上传、文本布局、软件栅格化、`play_multi` 组装和会话编码仍可能
落在 owner/UI 路径。成熟桌面宿主拥有更长时间积累的字体、DPI、无障碍和设备兼容
性，这些需要实机验收而不能从 Rust 单元测试推出。

## 6. 存档互操作和验证强度

当前验证已经覆盖：

- Glulx 文件头、长度、checksum、地址模式、栈帧、局部变量和非法内存；
- `CMem`/`UMem`、`Stks`、`MAll`、重复注释/扩展块、错误身份和损坏输入；
- Glulxe/Adventure 双向 IFZS 样本、Inform 加速 1--13、双精度和长 Huffman 字符串；
- 多窗口、Unicode、资源流、共享文件流、输入终止键、日期时间、图像和声音；
- Linux GUI/TTY 冒烟及当前资源、内存和 workload 工具。

`tools/check-reference.py` 的参数接受真实的参考解释器、候选程序和 fixture 目录，
并将临时合成故事放在临时目录中；参考实现验证命令见
[验收记录](glulx-validation.ZH.md)。这已经足以把 Glulxe 作为日常 oracle，但还
不是成熟实现级别的完整回归矩阵。

仍需补强的证据按优先级排序如下：

| 优先级 | 需要补的证据 | 原因 |
| --- | --- | --- |
| P0 | 同一输入脚本同时运行 Glulxe、Git 和 glulx-rs，固定等价的 headless Glk 合同，比较事件序列、输出、退出码和 IFZS | 区分 VM 差异与宿主差异 |
| P0 | 官方/社区故事的长路线、保存后恢复、重启、undo、计时器、音频通知和多窗口流程 | 合成故事不能覆盖长期状态交互 |
| P1 | 固定字体文件、DPI、窗口尺寸和音频设备的 GUI 对照 | 文本换行、字距、图片缩放和设备延迟属于宿主行为 |
| P1 | Windows/macOS 的 CLI、TTY、GUI、文件、字体和声音验收 | 当前主要运行证据集中在 Linux |
| P2 | 真实故事 profile，并与 Git 的 block compiler 成本模型对照 | 决定是否引入 block cache，而不是凭感觉优化 |

## 工程判断

当前 `glulx-rs` 不需要为了“看起来像成熟实现”立即移植 Git 的动态编译器，
也不应把 Glulxe 的 C 代码嵌入长期 VM 核心。更合理的边界是：

1. 继续把 Glulxe 作为行为和存档 oracle，把 Git 作为执行器/缓存设计参照。
2. 把 `check-reference.py` 从单一 Glulxe 路径扩展为可插入多个解释器和 Glk provider
   的矩阵，但保持故事、输入和输出记录可复现。
3. 先完成真实游戏的性能归因；只有 dispatch 确实占主导时，才设计带失效协议的
   decoded block cache。RAM 自修改、Glk 边界、输入等待和 debug trap 必须保留。
4. 把 mmap/惰性内存作为大故事启动和峰值驻留问题单独评估，不与 undo 页表优化混为
   一个项目。
5. 当 Web、远程或第二种原生宿主成为真实需求时，再把当前 VM 内的 Glk 状态和
   provider 交互抽成稳定的 host 接口；现在的单一 owner 模型仍适合现有桌面/TTY。

## 来源索引

本仓库：

- [VM、opcode、Glk dispatcher](../src/vm.rs)
- [内存与 dirty-page undo](../src/memory.rs)
- [故事、Blorb 和资源索引](../src/story.rs)
- [IFZS 存档](../src/vm/save.rs)
- [桌面会话](../src/vm/session.rs)
- [事件与输入](../src/vm/events.rs)
- [播放器时间片与 worker](../src/app.rs) / [媒体 worker](../src/app/media.rs)
- [兼容性说明](compatibility.ZH.md) / [验收记录](glulx-validation.ZH.md)
- [参考实现检查工具](../tools/check-reference.py)

外部项目：

- [Glulxe repository](https://github.com/erkyrath/glulxe/tree/56ab8743bab565de307bd892c555d8d8897ed517)
- [Glulxe README](https://github.com/erkyrath/glulxe/blob/56ab8743bab565de307bd892c555d8d8897ed517/README.md)
- [Glulx specification](https://eblong.com/zarf/glulx/Glulx-Spec.html)
- [Glk 0.7.6 specification](https://eblong.com/zarf/glk/Glk-Spec-076.html)
- [Git repository](https://github.com/DavidKinder/Git/tree/8f5604e10c6194f7d0a6222491eaeb236a70a874)
- [Git README](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/README.txt)
- [Gargoyle interpreter build](https://github.com/garglk/garglk/blob/9597add4091e5aaf6ebc31399b049158e12ca565/terps/CMakeLists.txt)
