# glulx-rs、GarglK 与 David Kinder Git：固定提交源码差异

复核日期：2026-09-10。跨项目结论使用下表固定的源码提交；当前实现状态以 `71e6889` 说明。外部项目结论来自固定提交的源码检查，不代表当天 upstream 的最新状态，也不等同于同一构建配置下的运行时基准。

本文按三个项目的实际职责比较固定基线的实现：`glulx-rs` 同时提供 Glulx VM、Glk 状态和 eframe 播放器；Git 只提供 Glulx VM，必须链接 Glk；GarglK 提供 Glk 桌面宿主、资源处理和启动器，并在本次基线中集成 Git 1.3.8。

## 固定比较基线

glulx-rs 的跨项目比较代码引用固定到 `4eccf8b`，不随当前分支推进而改变。GarglK 和 Git 的行为描述同样只适用于各自表中的提交；构建宏、宿主后端、字体、DPI、音频设备和操作系统会影响运行时结果。

| 项目 | 固定源码基线 |
| --- | --- |
| glulx-rs | [`4eccf8b`](https://github.com/KagurazakaNyaa/glulx-rs/commit/4eccf8b527349d3ae226be44194f1925477f94ee)，当前播放器与 VM |
| GarglK | [`9597add`](https://github.com/garglk/garglk/commit/9597add4091e5aaf6ebc31399b049158e12ca565)，2026-09-07 master |
| David Kinder Git | [`8f5604e`](https://github.com/DavidKinder/Git/commit/8f5604e10c6194f7d0a6222491eaeb236a70a874)，master，版本 1.3.9 |

GarglK 的 CMake 在这个提交中把 vendored Git 标为 1.3.8，并为它启用 `USE_INLINE`，按配置追加 `GIT_NEED_TICK`；不能把它和 Git master 1.3.9 或 Git 的其他构建选项当成同一个二进制。

## 当前仓库状态

当前实现状态 `bad16fb` 在固定比较基线之上还包含以下实现变化：

| 提交 | 当前变化 |
| --- | --- |
| `5c3d20e` | 故事映像和 `Memory.initial` 使用 `Arc` 共享；文件加载转移已拥有的 Blorb 字节，减少大故事启动时的复制。 |
| `1daf7bb` | 修复画布性能测试中的 Clippy 类型转换警告。 |
| `1a27dd4` | undo 预算按保留页、栈字节和堆记录数据计费，不再把共享故事映像和当前 VM 地址空间计入单次快照预算。 |
| `ebab7cb` | 超预算 undo 候选在构造完整页表前按页数上限拒绝，限制临时分配峰值。 |
| `71e6889` | VM 只为 RAMSTART 之后的可写区分配当前内存，ROM 直接从共享故事映像读取。 |
| `bad16fb` | headless workload 将诊断 heartbeat、VM 指令数、阶段耗时和 decode 命中率写入基准 JSON。 |

当前本地验证为：`272` 个库测试通过、`6` 个 CLI 测试通过、`6` 个性能测试忽略；fmt、全目标 Clippy 和 release 构建通过。真实大故事基准仍显示加载后的主要成本是故事容器、可执行映像、VM 内存和媒体资源的驻留副本，尚未引入 mmap 或 block compiler。

## 结论

1. **执行器仍是最大差异。** glulx-rs 逐条执行大型 Rust `match`，并以 2048 项固定直接索引缓存保存 ROM 指令的操作码、模式和立即数；它跳过了重复解码，但没有 Git 那种把一段指令编译为内部代码的 block compiler、peephole 重写或 native/JIT 后端。Git 用 Glulx 地址哈希表查找编译块，未命中时编译并按运行次数压缩代码缓存。
2. **undo 的实际页复制已经接近 Git。** glulx-rs 以 256 字节页记录相对故事初始状态的差异，并在相邻记录间共享 `Arc` 页；写入会标记 dirty pages，页快照只比较这些页，栈、heap 元数据和页表仍按记录保存。当前 `saveundo` 按候选快照实际保留的页、栈字节和 heap 记录数据计费；共享故事映像和当前 VM 地址空间不计入预算。Git 为每个 RAM 页保留指针表，未改变页指向初始映像或上一条记录，改变页才分配副本。
3. **活动 VM/Glk 的所有权仍是单一 owner。** glulx-rs 中正在执行的 VM、Glk 状态、窗口视图和 eframe UI 在同一逻辑线程；故事加载 worker 可以在安装前构造新的 `Vm` 并初始化音频，但不会与已安装的 VM 并发共享。其他后台线程负责图片解码、音频资源准备、翻译和诊断心跳。GarglK 的 launcher 通过 `QProcess` 隔离解释器进程，但 Git VM 与该进程内的 Qt Glk 仍由同一解释器线程驱动。
4. **宿主大块工作已部分异步。** GUI 故事读取、VM 建立、图片解码、采样/MOD 和普通 SONG 音频准备已通过带请求 ID 的 worker 返回；纹理上传、软件画布栅格化、文本布局、变化窗口复制和桌面会话序列化仍在 UI 线程。`play_multi` 为保持多声道同一采样帧起播，仍在 VM owner 内同步准备其批量音源。
5. **内存策略是 glulx-rs 的独有能力。** 游戏地址空间、进程硬上限、undo、图形缓存、文本图片、解码图片、音频资源和 SONG PCM 分开设额度。固定 MiB 或百分比都可以写入配置，命令行覆盖只对本次运行生效；Linux 使用 cgroup 上限或 `MemAvailable`，Windows 使用 `GlobalMemoryStatusEx` 的可用物理内存，并可用 `RLIMIT_AS`/Windows Job Object 设置进程上限。
6. **排版仍不是同一算法，但窄斜体回归已修复。** glulx-rs 使用 egui `LayoutJob`/`Galley` 和两个常规字体族，斜体由 egui 的 oblique 标志绘制，加粗用轻微偏移重复绘制；当前版本还会修正窄斜体字形四边形自相交的问题，并同时覆盖文本缓冲区和网格。GarglK 为比例/等宽、普通/斜体、粗体/粗斜体建立八种 FreeType `FontFace`，并缓存字形、字距和缺字替换。Git 不负责排版。

## 执行器与时间片

glulx-rs 的 [VM 执行路径](https://github.com/KagurazakaNyaa/glulx-rs/blob/4eccf8b527349d3ae226be44194f1925477f94ee/src/vm.rs)先轮询事件，再调用 `step`。`step` 解码操作码、最多八个操作数，执行整数、字符串、Glk、浮点、双精度和加速函数。ROM 地址经过 [decoded cache](https://github.com/KagurazakaNyaa/glulx-rs/blob/4eccf8b527349d3ae226be44194f1925477f94ee/src/vm.rs) 的固定索引；RAM 地址不缓存，以保留自修改代码语义。缓存不会跨桌面会话序列化，命中和未命中计数会出现在诊断摘要中。

播放器的 [VM 时间片](https://github.com/KagurazakaNyaa/glulx-rs/blob/4eccf8b527349d3ae226be44194f1925477f94ee/src/app.rs)每次最多执行 1024 条指令，并在批次结束时检查约 8 ms、状态变化或呈现版本变化。这个边界保证窗口可以重绘，但单个重型 opcode 或一个批次仍可能超过目标时间；长时间只计算、不产生 Glk 边界的游戏会跨多个 eframe 周期完成一次操作。

Git 的 [解释器入口](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/terp.c)把 PC、栈帧、局部变量和值栈放在局部寄存器/指针中，用 `NEXT` 跳到内部标签。它的 [代码块查找](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/compiler.h)通过地址哈希表命中已编译块，否则进入 [编译和缓存压缩](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/compiler.c)。`terp.c` 同时包含 direct-threading 分支，但是否启用取决于构建宏；GarglK 基线的 `GIT_MACROS` 只有 `USE_INLINE` 和可选 `GIT_NEED_TICK`。

## 内存、限制与 undo

| 项目 | glulx-rs | Git | GarglK |
| --- | --- | --- | --- |
| 故事内存 | `Memory` 保存当前字节和初始故事映像，地址访问带边界/ROM 检查 | `gInitMem` 保存只读映像，`gMem` 保存可写的 `EndMem` | 不拥有 Glulx 地址空间，由 Git 负责 |
| 游戏上限 | 默认 1 GiB；配置支持固定 MiB 或启动内存百分比，按 256 字节对齐 | 地址值为 32 位；端口通常只给固定缓冲区，不提供同等策略 | 没有独立的 Glulx 内存策略 |
| 进程上限 | Linux `RLIMIT_AS`，Windows Job Object；启动时设置且保留更严格的继承上限 | 由宿主/操作系统决定 | 由 launcher/解释器进程和操作系统决定 |
| undo | 256 字节差分页，未改变页与前一条记录共享；栈、heap 和 protection 边界单独保存 | 256 字节页指针表；未改变页共享初始映像或上一记录，改变页复制 | 宿主不实现 Git 的 undo 数据结构 |

glulx-rs 的 [资源预算](https://github.com/KagurazakaNyaa/glulx-rs/blob/4eccf8b527349d3ae226be44194f1925477f94ee/src/memory.rs)默认值为 undo 256 MiB、图形缓存 512 MiB、文本图片 256 MiB、单图解码 256 MiB、音频资源 256 MiB、SONG PCM 128 MiB。`Budget` 可以是整数 MiB 或 `1%` 到 `100%`；配置文件与命令行分别由 [memory policy](https://github.com/KagurazakaNyaa/glulx-rs/blob/4eccf8b527349d3ae226be44194f1925477f94ee/src/memory_budget.rs) 解析，命令行覆盖不写回配置。

页快照以故事初始 RAM 和扩展区零值为基线，只保存不同页；恢复时重建目标长度、覆盖保存页，并恢复当前 `protection` 区域。写入、扩容、restart 和 restore 会更新 dirty-page 集合，成功保存 undo 后清空它。旧桌面会话中的完整 `Memory` 会在 [session validation](https://github.com/KagurazakaNyaa/glulx-rs/blob/4eccf8b527349d3ae226be44194f1925477f94ee/src/vm/session.rs) 中迁移为页表。这个格式只影响桌面会话和 VM undo，不改变可移植 IFZS 的 `CMem`/`Stks`/`MAll` 合同；页表仍是 Rust `BTreeMap`，并且候选页快照在预算判断前需要构造，后续可继续优化其临时峰值分配。

Git 的 [Windows 端口](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/git_windows.c)默认使用 256 KiB 代码缓存和 2 MiB undo，并用 `CreateFileMapping`/`MapViewOfFile` 传入游戏映像。Git README 将 `cacheSize` 定义为重编译代码缓存，将 `undoSize` 定义为 undo 总预算；这两个数不能直接与 glulx-rs 的多类 MiB 预算相加。GarglK CMake 的 Git 目标实际编译 `git_unix.c`，所以这个 mmap 结论只适用于 Git 的独立 Windows 端口，不适用于 GarglK 当前构建。

## Glk dispatcher 与对象边界

glulx-rs 的 [Glk dispatcher](https://github.com/KagurazakaNyaa/glulx-rs/blob/4eccf8b527349d3ae226be44194f1925477f94ee/src/vm.rs)直接匹配 selector。参数先进入最多 64 个元素的栈上缓冲，超出才分配堆数组；窗口、流、fileref、请求和事件分别由 Rust 容器管理，句柄是可序列化整数。标准窗口/流、文本输出、输入事件、图像、声音、Unicode、日期和 style hint 都在同一个 VM 模块中处理；未知 selector 记录后返回零。仍有大参数 fallback、函数参数组装和 `play_multi` 临时向量等分配点。

Git 的 [glkop ABI bridge](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/glkop.c)根据 Glk prototype 表转换整数、数组和 opaque 对象。它对常用字符输出、当前流和大小写操作保留快速路径；其余调用经过临时数组、对象指针转换、`gidispatch_call` 和结果回写。对象 ID 到 `window_t`、`stream_t`、`fileref_t`、`schanid_t` 的映射使用每类 31 桶哈希表，并维护 retained arrays。

GarglK 把 [Glk dispatch table](https://github.com/garglk/garglk/blob/9597add4091e5aaf6ebc31399b049158e12ca565/garglk/cheapglk/gi_dispa.c)和各类 `cheapglk` 实现编译进 `garglk-common`。因此 Git 的 `glkop.c` 是 VM 到 Glk 的桥，GarglK 的 `gi_dispa.c` 是宿主 ABI 的 dispatcher，两者是配套层而不是互相替代。

| 能力 | glulx-rs | GarglK + Git |
| --- | --- | --- |
| Glk 版本 | 按 host 能力报告 Glk 0.7.6 | `cggestal.cpp` 报告 Glk 0.7.6 |
| 标准 selector | 在 VM 中直接实现并做 Rust 边界检查 | 由 Git prototype 和 GarglK dispatch table 分工实现 |
| 图形/声音 | 图形播放器声明图片、缩放、超链接和音频能力；终端 host 关闭这些能力 | 能力由 GarglK 编译模块、配置和设备后端决定 |
| 扩展 | 没有 GarglK 专有 selector | 包含 `GLK_MODULE_GARGLKTEXT`、overlay、像素窗口等宿主扩展 |
| 图形字符输入 | 不报告支持 | GarglK 的 `cggestal.cpp` 也返回不支持 |

glulx-rs 的整数句柄和可序列化窗口树更适合 Rust 会话边界；Git/GarglK 的指针注册表更适合 C ABI 和即时宿主调用。不能只根据 selector 数量判断两者行为完全相同，还要分别验证 host 能力、参数边界和媒体后端。

## Glulx 版本与扩展

glulx-rs 的 [故事头解析](https://github.com/KagurazakaNyaa/glulx-rs/blob/4eccf8b527349d3ae226be44194f1925477f94ee/src/story.rs)接受 `0x00020000..=0x000301ff`，并实现 1、2、4 字节局部变量、双精度和 3.1.3 undo/加速接口。Git 的 [版本检查](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/git.c)同时接受 1.0、2.0、3.0 和 3.1；Git README 仍记录 1/2 字节局部变量不支持、direct search key 必须正好 4 字节。

Git upstream 1.3.9 在 [`5ae06ca`](https://github.com/DavidKinder/Git/commit/5ae06ca59375c9e12c617d7087651753ff9dadbe)增加 `-0x80000000 / -1` 的 `div/mod` 溢出检查；当前 glulx-rs 的整数除法和取余仍使用 wrapping 语义，这是需要单独验证的兼容性差异。Git upstream 还在 [`4636165`](https://github.com/DavidKinder/Git/commit/46361650e8c9a4e3acbf1726e5d5e94d027d11b7)删除了旧的 `@git_setcacheram` 与 `@git_prunecache`；它们可能仍存在于 GarglK 所集成的 [Git 1.3.8 vendor](https://github.com/garglk/garglk/commit/af7e229a39c5af05928fba12f3f5a9e605370c2f)，但不是当前 upstream Git 的标准能力。

## 事件、输入与线程

glulx-rs 的 [event state machine](https://github.com/KagurazakaNyaa/glulx-rs/blob/4eccf8b527349d3ae226be44194f1925477f94ee/src/vm/events.rs)用 `pending_select`、请求表和 FIFO 事件队列表示等待。`select_poll` 只取 Arrange、Redraw、SoundNotify 和 Timer 等内部事件；玩家输入由 eframe 事件转成 VM 的行/字符/鼠标/超链接接口。UI 提交输入后，下一次 `logic` 才继续 VM 时间片。

GarglK 的 [event list](https://github.com/garglk/garglk/blob/9597add4091e5aaf6ebc31399b049158e12ca565/garglk/event.cpp)在普通 select 中取队首，在 poll 中查找允许的内部事件。Qt 的 [select loop](https://github.com/garglk/garglk/blob/9597add4091e5aaf6ebc31399b049158e12ca565/garglk/sysqt.cpp)先处理 Qt 事件，没事件时调用 `QEventLoop::WaitForMoreEvents`，再把事件交给 Glk。GarglK 的 launcher 通过 [QProcess](https://github.com/garglk/garglk/blob/9597add4091e5aaf6ebc31399b049158e12ca565/garglk/launchqt.cpp)启动解释器；解释器内部没有额外的 VM worker。

图片、音频和翻译 worker 不直接访问正在运行的 `Vm`、`egui::Context` 或 Glk 对象，只返回带 ID 的结果；`StoryLoadWorker` 是例外，它在后台构造一个尚未安装的新 `Vm`，完成后再按请求 ID 交给 owner。这样可以把可阻塞的文件/媒体准备移出 UI，又不会让后台线程重排已运行 VM 的输入、Glk 调用或 undo 边界。剩余延迟主要来自长 VM 时间片、文本排版、纹理上传/软件栅格化以及仍同步的会话保存和 SONG 组装。

## 媒体、故事和资源

| 路径 | glulx-rs 当前实现 | GarglK/Git 参考实现 |
| --- | --- | --- |
| 故事打开 | GUI 使用 `StoryLoadWorker` 完成文件读取、Blorb 解析、资源挂载和 VM 建立；只安装最新请求 | Git API 接收调用方提供的内存指针或 Glk stream；独立 Windows 端口使用文件映射，GarglK CMake 使用 `gitWithStream` 路径 |
| 图片 | VM 校验并读取图片尺寸，同时把编码资源字节复制到图形请求；完整 RGBA 解码在 `ImageDecodeWorker`，UI 线程创建纹理并按顺序绘制 | [GarglK image loader](https://github.com/garglk/garglk/blob/9597add4091e5aaf6ebc31399b049158e12ca565/garglk/imgload.cpp)在宿主线程读取/解码，并按引用计数保留原图和缩放图 |
| 采样/MOD 音频 | `AudioDecodeWorker` 准备采样和 MOD source；VM owner 负责声道状态、取消和通知 | Git 把声音交给所链接的 Glk；GarglK 的 Qt/SDL 后端在宿主解释器线程准备资源，设备回调另有线程 |
| SONG | 普通播放把同一 Blorb 资源表的 AIFF 样本快照交给 `AudioDecodeWorker`；`play_multi` 为保持采样帧对齐仍同步组装 | GarglK/Git 的对应 SONG/音频实现属于宿主和解释器构建，不与 glulx-rs 的 Rust 资源表共享 |
| 缓存 | 图形画布和文本图片分别有 LRU/预算；解码结果不进入 VM 会话 | GarglK 以 `picstore` 和引用计数管理图片；Git 主要依赖宿主资源和内部代码/undo 缓冲 |

glulx-rs 的 [Story](https://github.com/KagurazakaNyaa/glulx-rs/blob/4eccf8b527349d3ae226be44194f1925477f94ee/src/story.rs)保留故事映像、可选原始容器和外部资源内容，以便会话脱离原文件恢复；这会产生多份字节副本。图形请求还会暂存编码图片字节，直到 UI worker 完成解码。Git README 明确建议在系统支持时 mmap，以减少启动复制并更快开始执行；GarglK 当前 CMake 的 Git 集成不使用这个独立 Windows 端口。

当前 worker 都是单线程队列。图片和非 SONG 音频准备使用容量为 2 的非阻塞任务队列，队列满时 owner 保留请求并在后续宿主周期重试，不把发送端阻塞在 UI 上；图形请求仍一次只等待一个未完成图片，以保持 Fill/Clear/Close 顺序。故事加载使用条件变量队列，只保留一个最新待处理任务；已经开始的旧任务仍只能在阶段边界丢弃结果。迟到的图片/音频结果通过请求 ID 丢弃，避免写入当前故事状态。

## 文本、字体和图形

glulx-rs 的 [text buffer](https://github.com/KagurazakaNyaa/glulx-rs/blob/4eccf8b527349d3ae226be44194f1925477f94ee/src/app/text_buffer.rs)把 `TextRun` 分成段落、单词、空格、CJK 字符、换行和图片，使用 LayoutJob/Galley 计算可见布局，并按窗口内容修订、宽度、字号、链接色和像素比例缓存。网格按字符、样式、链接和字体参数缓存 galley；变化窗口才复制新的 `WindowView`。图形窗口保留 Fill/Image 操作，硬件路径交给 OpenGL，软件路径或会话保存时才栅格化像素。

GarglK 的 [text reflow](https://github.com/garglk/garglk/blob/9597add4091e5aaf6ebc31399b049158e12ca565/garglk/wintext.cpp)按滚屏行保存字符、属性、边缘图片和 flow break；只有像素宽度/高度变化才重排，重绘时跳过未 dirty 行。其 [FreeType renderer](https://github.com/garglk/garglk/blob/9597add4091e5aaf6ebc31399b049158e12ca565/garglk/draw.cpp)为八种 FontFace 组合缓存轮廓、字形、kerning 和缺字替换，可使用真实 italic/bold 文件，也可对轮廓应用 oblique/embolden 变换。

因此两边的字号相同也不意味着换行相同。当前 glulx-rs 与 GarglK 的可见差异包括：

- egui 的 fallback/shaping 与 FreeType substitution 的字体选择顺序不同；
- glulx-rs 的斜体主要依赖 egui oblique 标志，GarglK 还会选择真实 italic/bold-italic face；
- glulx-rs 在变化窗口内复制完整 runs/grid，GarglK 按 dirty 行重绘；
- glulx-rs 的 retained GPU 画布与 GarglK 的 CPU RGB 像素缓冲在缩放、透明和重绘时机上不同。

这些是排版/宿主实现差异，不应通过强行共享字体名称来推断兼容性。验收应固定字体文件、字号、DPI、窗口宽度和颜色，分别比较普通、粗体、斜体、粗斜体、CJK fallback、kerning、段落缩进、对齐和边缘图片。

## 存档与会话

glulx-rs 的 [portable save](https://github.com/KagurazakaNyaa/glulx-rs/blob/4eccf8b527349d3ae226be44194f1925477f94ee/src/vm/save.rs)使用 IFZS 的 `CMem`、`Stks` 和 `MAll`，校验完成后才替换 VM 状态；桌面会话还保存 VM、transcript、输入和 CPU 画布像素。桌面快照估算超过 16 MiB 时跳过自动保存，以避免把大型序列化工作放进 UI 回调；这条会话编码路径仍是同步的。

Git 的 `savefile.c`/`saveundo.c`处理可移植存档、栈和页表，不保存 GarglK 的完整桌面窗口。GarglK 可以提供宿主自动存档和文件对话框，但它们不改变 Git 的 Glulx 内存合同。

## 当前优化状态

证据栏区分实现源码检查、自动化测试、Linux GUI 验收和未完成的跨项目/跨平台实测；源码检查不等同于性能或像素级兼容性证明。

| 顺序 | 当前状态 | 证据 | 仍与参考项目不同的边界 |
| --- | --- | --- | --- |
| 可观测性 | 已有 `vm-slice`、UI、publish、布局、图形上传、音频准备、undo 和会话阶段计时 | 源码检查、单元测试、诊断日志 | 输入事件到每个阶段的跨线程 trace 仍可进一步细化 |
| 异步宿主工作 | 故事、图片、采样/MOD/普通 SONG 音频准备已使用有序且有界的 worker | worker 源码、单元测试、Linux GUI 工具 | 纹理上传、布局、软件栅格化、`play_multi` SONG 批处理和会话编码仍在 owner/UI |
| 指令缓存 | ROM decoded cache 已实现，RAM 代码不缓存 | `decoded_cache_reuses_rom_instruction_metadata` 测试、release 微基准 | 没有 Git 风格 block compiler、peephole 或 JIT；只有在固定基准证明 VM dispatch 是主要瓶颈后才考虑，且必须保留 RAM 自修改和 Glk 边界语义 |
| undo | 256 字节页差分、dirty-page 跟踪、共享页、旧会话迁移和按快照 payload 计费已实现 | Memory/session/conformance 测试、`undo_budget_counts_snapshot_pages_not_the_story_image`、`limited_page_snapshots_reject_dense_dirty_memory`、源码检查 | 页表是 Rust `BTreeMap`，不是 Git 的原始指针数组；很大的 dirty set 仍需 profile |
| 工程质量 | 当前全目标 Clippy、格式检查和 release 构建通过 | 当前实现状态 `bad16fb` 的本地验证 | Windows/macOS CI 与实机 GUI 仍需分别记录 |
| Glk/呈现热路径 | 参数使用固定小缓冲，翻译关闭时不捕获，transcript 最多保留 4 MiB/100,000 行且显示采用虚拟行，变化窗口发布有界 | Glk conformance、Linux GUI/终端工具、单元测试 | 逐字符 Glk 输出和变化窗口内的完整向量仍不同于 GarglK 的 dirty 行 |
| 字体/排版 | 已有字体 fallback、样式 hint、布局缓存和 CJK 分段测试；`4eccf8b` 修复了文本/网格窄斜体字形四边形自相交 | 字体/布局单元测试、Linux GUI 验收 | 真实八种字体组合、FreeType 字距、Windows/macOS 和跨项目像素差分仍待实机验收 |

## 来源

glulx-rs：

- [VM 与 Glk](https://github.com/KagurazakaNyaa/glulx-rs/blob/4eccf8b527349d3ae226be44194f1925477f94ee/src/vm.rs)
- [事件](https://github.com/KagurazakaNyaa/glulx-rs/blob/4eccf8b527349d3ae226be44194f1925477f94ee/src/vm/events.rs)
- [内存与预算](https://github.com/KagurazakaNyaa/glulx-rs/blob/4eccf8b527349d3ae226be44194f1925477f94ee/src/memory.rs)
- [会话校验](https://github.com/KagurazakaNyaa/glulx-rs/blob/4eccf8b527349d3ae226be44194f1925477f94ee/src/vm/session.rs)
- [播放器与 worker](https://github.com/KagurazakaNyaa/glulx-rs/blob/4eccf8b527349d3ae226be44194f1925477f94ee/src/app.rs)
- [媒体 worker](https://github.com/KagurazakaNyaa/glulx-rs/blob/4eccf8b527349d3ae226be44194f1925477f94ee/src/app/media.rs)
- [文本布局](https://github.com/KagurazakaNyaa/glulx-rs/blob/4eccf8b527349d3ae226be44194f1925477f94ee/src/app/text_buffer.rs)
- [文本网格](https://github.com/KagurazakaNyaa/glulx-rs/blob/4eccf8b527349d3ae226be44194f1925477f94ee/src/app/text_grid.rs)
- [字体](https://github.com/KagurazakaNyaa/glulx-rs/blob/4eccf8b527349d3ae226be44194f1925477f94ee/src/app/fonts.rs)
- [故事和资源](https://github.com/KagurazakaNyaa/glulx-rs/blob/4eccf8b527349d3ae226be44194f1925477f94ee/src/story.rs)
- [声音](https://github.com/KagurazakaNyaa/glulx-rs/blob/4eccf8b527349d3ae226be44194f1925477f94ee/src/vm/sound.rs)

David Kinder Git 1.3.9：

- [README](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/README.txt)
- [解释器](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/terp.c)
- [代码块编译器](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/compiler.c)
- [代码块查找接口](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/compiler.h)
- [内存](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/memory.c)
- [undo](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/saveundo.c)
- [Glk ABI](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/glkop.c)
- [Windows 端口](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/git_windows.c)
- [能力查询](https://github.com/DavidKinder/Git/blob/8f5604e10c6194f7d0a6222491eaeb236a70a874/gestalt.c)

GarglK：

- [GarglK CMake](https://github.com/garglk/garglk/blob/9597add4091e5aaf6ebc31399b049158e12ca565/garglk/CMakeLists.txt)
- [Git 集成 CMake](https://github.com/garglk/garglk/blob/9597add4091e5aaf6ebc31399b049158e12ca565/terps/CMakeLists.txt)
- [Glk dispatcher](https://github.com/garglk/garglk/blob/9597add4091e5aaf6ebc31399b049158e12ca565/garglk/cheapglk/gi_dispa.c)
- [能力查询](https://github.com/garglk/garglk/blob/9597add4091e5aaf6ebc31399b049158e12ca565/garglk/cheapglk/cggestal.cpp)
- [事件队列](https://github.com/garglk/garglk/blob/9597add4091e5aaf6ebc31399b049158e12ca565/garglk/event.cpp)
- [Qt select](https://github.com/garglk/garglk/blob/9597add4091e5aaf6ebc31399b049158e12ca565/garglk/sysqt.cpp)
- [窗口与文本](https://github.com/garglk/garglk/blob/9597add4091e5aaf6ebc31399b049158e12ca565/garglk/wintext.cpp)
- [字体与绘制](https://github.com/garglk/garglk/blob/9597add4091e5aaf6ebc31399b049158e12ca565/garglk/draw.cpp)
- [图片加载](https://github.com/garglk/garglk/blob/9597add4091e5aaf6ebc31399b049158e12ca565/garglk/imgload.cpp)
- [launcher](https://github.com/garglk/garglk/blob/9597add4091e5aaf6ebc31399b049158e12ca565/garglk/launchqt.cpp)

这些源码链接固定到本次复核提交；文档不依赖本机 checkout 路径，也不把本地构建目录当成参考项目来源。
