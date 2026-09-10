# glulx-rs、GarglK 与 David Kinder Git 的源码差异复核

复核日期：2026-09-10。

本文把 [glulx-rs](https://github.com/KagurazakaNyaa/glulx-rs)、[GarglK](https://github.com/garglk/garglk) 和 [David Kinder Git](https://github.com/DavidKinder/Git) 1.3.8 放在同一张表里比较。比较对象不是三个同层次的程序：`glulx-rs` 同时包含 VM、Glk 状态和 eframe 播放器；Git 是 Glulx VM，必须依赖 Glk；GarglK 是 Glk/桌面宿主和启动器，并把 Git 编译成独立解释器。

源码基线如下：

| 对象 | 上游仓库与基线 |
| --- | --- |
| `glulx-rs` | [`335b526`](https://github.com/KagurazakaNyaa/glulx-rs/commit/335b526aa8b6dc0e2dab40892c65ea61b697a99e)，`fix: enable graphics hyperlink input` |
| GarglK 宿主 | [`garglk/garglk@9597add`](https://github.com/garglk/garglk/commit/9597add4091e5aaf6ebc31399b049158e12ca565) |
| Git | [David Kinder Git `master`](https://github.com/DavidKinder/Git/tree/master)，版本 1.3.8 |

## 结论

1. **指令执行路径是最大结构差异。** Rust 每条指令都经过 `fetch_opcode`、`fetch_operands`、`load_operand` 和一个大型 `match`；现在对 ROM 区域使用固定直接索引的 decoded cache，命中时跳过操作码、模式和立即数读取。Git 仍把一段 Glulx 指令编译为内部代码块，地址到代码块通过哈希表复用，并可用 peephole 优化；Rust 仍没有等价的 native block compiler 或 JIT。
2. **undo 是第二个明确的性能热点。** Rust 现在按 256 字节页保存差异，不变页在相邻 undo 记录间共享 `Arc`；栈和 heap 元数据仍按记录保存。Git 使用相同页大小和共享指针策略，因此两者的 undo 复制成本不再由完整游戏内存决定。
3. **两个桌面架构都没有把 VM 放进独立执行线程。** GarglK 启动器通过 `QProcess` 启动独立的 Git 进程，但该进程内的 Git VM 和 Qt Glk 仍在同一线程；阻塞 `glk_select` 时由该线程泵 Qt 事件。Rust 的 eframe 逻辑和 UI 也在同一线程，只在每一帧最多运行约 8ms 的 VM 时间片。把 VM 改成线程所有权模型会改变输入和 Glk 调用边界，不能只在现有对象上随意加锁。
4. **Rust 的剩余同步宿主工作仍可能遮住 VM 优化。** 故事读取和 VM 建立、图片解码、采样音频准备已经由有序 worker 承担；纹理上传、软件画布栅格化、文本布局、窗口视图复制和会话序列化仍在 eframe 线程。点击事件在 UI 回调里提交给 VM，VM 通常要等下一次 `logic` 才继续执行。
5. **翻译关闭时不再捕获正文事件。** `run_vm` 只在开关启用时建立 `TextBufferEvent`，关闭时直接丢弃待捕获事件；翻译窗口仍对已记录的回合做有界显示。GarglK 的 `gli_translation_append` 在翻译关闭时也立即返回（`garglk/translation.cpp:107-111`）。
6. **渲染实现的差异会影响当前换行和斜体问题。** GarglK 使用 FreeType 的八种实际字体组合、字形/字距缓存和 CPU 像素缓冲；Rust 使用 egui `LayoutJob`/`Galley`，通过 `RichText::italics()` 和二次偏移绘制加粗。两者不是同一排版算法，不能只用字号相同来推断布局应相同。

## 一、执行引擎

### `glulx-rs`

`Vm::run_steps_until` 先调用一次 `poll_events`，然后逐条增加计数并调用 `step`（`src/vm.rs:714-730`）。`step` 在 `src/vm.rs:780-805` 解码操作码和操作数，之后进入覆盖整数、字符串、Glk、浮点和双精度指令的 `match`。操作码和地址模式由 `src/vm.rs:1467-1513` 读取；内存/栈操作还经过 `src/vm.rs:1534-1583` 的安全访问函数。

桌面播放器用 `run_vm_slice` 重复执行最多 1024 条指令，直到状态改变、呈现版本改变或经过约 8ms（`src/app.rs:1870-1883`）。每个 eframe 逻辑回调先执行这个切片，再处理翻译和其他宿主工作（`src/app.rs:1757-1774`）。这保证了界面有机会重绘，但也意味着每次用户操作至少经过一次 UI/logic 周期。

### Git

Git 在 `startProgram` 中把 PC、栈帧、局部变量和值栈保存在本地寄存器/指针变量，并用 `NEXT` 直接跳到下一个内部标签（`terps/git/terp.c:200-299`）。`getCode` 对 Glulx PC 做哈希查找，命中时增加代码块运行计数，未命中时调用 `compile`（`terps/git/compiler.h:93-107`）。

`compiler.c` 会把多个指令解析为代码块，记录分支和可跳转地址，并在块结束时把引用地址挂入哈希表（`terps/git/compiler.c:146-218`、`309-360`）。缓存空间不足时按运行计数压缩，必要时清空代码缓存（`terps/git/compiler.c:365-547`）。`peephole.c` 会合并相邻 load/store、算术和地址模式（`terps/git/peephole.c:13-135`）。因此 README 中的速度/缓存说明所指的 `cacheSize` 是**重编译后的内部代码缓存**，不是游戏内存。

这里存在一个容易误读的构建差异：Git 自带的旧 Windows Makefile 开启 `USE_DIRECT_THREADING`（`terps/git/Makefile.win:3-4`），但 GarglK 当前 CMake 给 Git 的宏只有 `USE_INLINE`，没有加入该宏（`terps/CMakeLists.txt:210-226`）。GarglK 的 Qt 构建还定义 `GARGLK_CONFIG_TICK` 并设置 `GARGLK_NEEDS_TICK`（`garglk/CMakeLists.txt:260-266`），于是 Git 构建会加入 `GIT_NEED_TICK`（`terps/CMakeLists.txt:213-215`）。在未启用 direct threading 的路径中，`terp.c:269-286` 的 `switch` 每次进入 `next` 前还会调用 `glk_tick`；Qt 的 `gli_tick` 每次只检查一个原子标志，但标志置位时会调用 `processEvents`（`garglk/sysqt.cpp:1044-1058`）。所以“上游 Git 的速度”和“GarglK CMake 产出的 Git”不是同一个执行路径。

### 性能含义

当前实现和后续边界如下：

- 先按 `vm-slice`、`ui`、`graphics`、文本布局、音频和 undo 分阶段测量；单看总 CPU 使用率不能区分阻塞等待、短促的单线程工作和锁/分配延迟。
- ROM 的按 PC 直接索引 decoded cache 已落地；RAM 代码仍逐次解码，因此自修改代码不需要额外失效协议。后续若实现更大的代码块缓存，仍不能直接复制 Git 的 C 标签指针。
- 翻译关闭时停止捕获。只有 `translation.enabled` 为真时才启用 `TextBufferEvent` 和回合累积；辅助窗口应对历史设置有界预算或虚拟化，避免正文长度直接决定每帧 widget 数量。开启翻译时保留现有回合边界和请求顺序。
- 如果未来缓存 RAM 代码，必须在 `setmemsize`、写入可能包含代码的 RAM、Git 风格的 cache-prune 扩展和 restart 时失效；这是 Rust 版本的安全边界。

## 二、内存、栈与 undo

| 项目 | `glulx-rs` | Git | GarglK |
| --- | --- | --- | --- |
| 故事 ROM/当前内存 | `Story`、`Memory` 和 `Vm` 分开；`Memory` 持有当前 `Vec<u8>` 和初始映像副本（`src/memory.rs:53-62`） | `gInitMem` 指向只读故事映像，`gMem` 是一份可写的 `EndMem` 缓冲（`terps/git/memory.c:7-14`、`63-75`） | 宿主负责文件/Blorb，实际 VM 内存属于 Git；GarglK 本身不拥有 Glulx 地址空间 |
| 地址访问 | `read8/16/32`、`write8/16/32` 做边界和 ROM/RAM 检查（`src/memory.rs:145-187`、`262-279`） | `memory.h` 的内联宏做范围检查，随后直接访问原始指针（`terps/git/memory.h:108-155`） | 由解释器完成 |
| 扩容 | 256 字节对齐、检查 `maximum`，`Vec::try_reserve_exact` 失败返回 false（`src/memory.rs:202-218`） | `realloc`，失败返回 1；只有非内部扩容时阻止活动 heap（`terps/git/memory.c:89-113`） | 无独立限制策略 |
| 默认游戏地址空间 | `MAX_MEMORY_SIZE = 1 GiB`（`src/memory.rs:3-4`），播放器可通过 `Budget` 设置固定 MiB 或启动内存比例 | Glulx 32 位地址值，端口没有同等的应用层上限 | 进程/解释器是否受限制由操作系统和启动器决定 |
| 栈局部变量 | 支持 1、2、4 字节局部变量，并检查帧格式和栈上限（`src/vm.rs:1598-1663`） | 只接受 4 字节局部变量；遇到 1/2 字节直接 fatal error（`terps/git/terp.c:363-380`） | 不参与 VM 栈实现 |

Rust 的 `saveundo` 按页记录当前内存与前一条快照的差异，并把 `self.stack.clone()`、`heap` 索引和页表放入 `UndoState`。恢复时以故事初始 RAM/扩展区为基线，再覆盖保存页并保留 protection 范围。预算通过 `trim_undo` 淘汰最旧记录，成本按实际拥有页和栈/heap 元数据估算。

Git 的 `saveUndo` 先复制完整栈和每个 RAM 页的指针表；第一条 undo 记录把未改变的页指向 `gInitMem`，改变的页才分配 256 字节副本，扩展内存页则单独保存（`terps/git/saveundo.c:47-110`）。后续记录只与上一条记录逐页比较，未改变页共享指针（`terps/git/saveundo.c:112-145`）。`deleteRecord` 在相邻记录或初始映像仍持有页时不重复释放（`terps/git/saveundo.c:308-381`）。这就是 Git 在小 undo 缓冲下仍能保存多步历史的主要原因。

Git 的运行端口默认值也明显更低：Windows Git 使用 256 KiB 代码缓存和 2 MiB undo（`terps/git/git_windows.c:66-67`）；Rust 当前默认 undo 预算为 256 MiB、代码没有缓存预算（`src/memory.rs:18-28`）。这两个数字不能直接相加比较，因为 Git 的 `undoSize` 是 C 结构/页/栈实际占用，Rust 的 `undo_mib` 是对完整快照数据的估算。

## 三、Glk dispatcher 与对象边界

### `glulx-rs`

Glk 调用从 Glulx opcode `0x130` 进入 `Vm::glk`（`src/vm.rs:1173-1177`）。`Vm::glk` 把参数从 Rust 栈弹到一个按调用大小分配的 `Vec<u32>`，然后按 selector 直接匹配窗口、流、文件、文本、事件、图像、声音、Unicode 和日期 API（`src/vm.rs:1930-1946`、`1947-2449`）。窗口/流/fileref/请求分别存在 `BTreeMap`、`HashSet` 和 `VecDeque` 中；对象 ID 就是 Rust 自己分配的整数，不经过 C 指针注册表。

未知 Glk selector 被记录并返回零（`src/vm.rs:2444-2449`）。Glk 版本及宿主能力由 `glk_gestalt` 按图形、终端、声音、字体和资源状态返回（`src/vm.rs:2479-2524`）。这种实现便于类型化错误和会话序列化，但每个调用仍有参数 `Vec` 和多次 map 查找，批量输出时还会逐字符走 `glk_write_char`（`src/vm.rs:1901-1904`）。

### Git + GarglK

Git 的 `glkop.c` 是 Glulx 参数到 C Glk ABI 的桥。常用 `stream_set_current`、`stream_get_current`、`put_char`、`put_char_stream`、大小写转换和 Unicode 字符输出走直接快速路径（`terps/git/glkop.c:313-368`）；其他调用读取 Glk prototype，经历计算参数空间、转换数组/结构、调用 `gidispatch_call`、把输出写回 Glulx 内存四个阶段（`terps/git/glkop.c:374-418`）。

该桥还维护：

- 每种 opaque Glk 类的 31 桶哈希表，用于把 Glulx 的整数 ID 映射回 `window_t`、`stream_t`、`fileref_t` 和 `schanid_t`（`terps/git/glkop.c:213-237`、`1089-1133`）。
- 临时字符数组、整数数组和对象指针数组；传入/传出方向由 prototype 的 `<`、`>`、`&` 标记控制（`terps/git/glkop.c:421-468`、`1182-1387`）。
- retained array 注册/注销，以便 Glk 库在调用之间保留数组引用（`terps/git/glkop.c:1389-1465`）。

GarglK 把官方 Glk 0.7.6 `gi_dispa.c` 和 prototype/function table 编译进 `garglk-common`（`garglk/CMakeLists.txt:112-125`）。表中覆盖窗口、流、文件、输出、事件、图片、声音、超链接、Unicode、行终止符、日期和 GarglK 文本扩展；功能按 `GLK_MODULE_*` 宏可选（`garglk/cheapglk/gi_dispa.c:188-345`）。因此这里的 dispatcher 不是 Git 的替代物：Git `glkop.c` 负责 VM ABI，GarglK `gi_dispa.c`/Glk 实现负责宿主 ABI，两者配套工作。

### 0.7.6 selector 快照

下表按当前源码的 selector 范围归纳标准覆盖。GarglK 的 `gi_dispa.c` 是宿主编译时的能力表；Git 的 `glkop.c` 只有在所链接的 Glk 库提供对应 prototype/函数时才会成功；Rust 则在自己的 `Vm::glk` 中直接处理这些 selector。

| 标准范围 | `glulx-rs` | GarglK + Git | 主要差异 |
| --- | --- | --- | --- |
| `0x0001-0x0005` | 直接处理 exit、interrupt/tick 占位、gestalt | `gi_dispa.c:188-194` 列出同组函数 | Git 的 `glkop.c` 对未知 prototype 报 fatal；Rust 未知 selector 记录并返回 0 |
| `0x0020-0x0030` | 窗口迭代、树、尺寸、布局、清屏、流和 sibling（`src/vm.rs:1964-2107`） | `gi_dispa.c:195-211` + `window.cpp` | GarglK 有 overlay/透明窗口扩展；Rust 用可序列化窗口树 |
| `0x0040-0x0068` | 流、文件和 fileref（`src/vm.rs:2108-2207`） | `gi_dispa.c:212-232` + `cgstream.cpp`/`cgfref.cpp` | Rust 文件提示状态留在 VM；GarglK 文件对话框属于 Qt 宿主 |
| `0x0080-0x00b3` | 输出、流读取、大小写、样式和 style hints（`src/vm.rs:2214-2266`） | `gi_dispa.c:233-249` + `cgstream.cpp`/`style.cpp` | Git 对常用字符输出走快速路径，Rust 逐字符更新文本 run |
| `0x00c0-0x00d6` | select/poll、行/字符/鼠标/定时器输入（`src/vm.rs:2289-2368`） | `gi_dispa.c:250-258` + `event.cpp`/`window.cpp` | GarglK 的 select 阻塞并泵 Qt；Rust 由 eframe logic 周期推进 |
| `0x00e0-0x00eb`、`0x00ec` | 图像查询、绘制、缩放、flow/erase/fill/background（`src/vm.rs:2369-2408`） | `gi_dispa.c:259-269` + `imgload.cpp`/`window.cpp` | GarglK 图像缓存和缩放在宿主线程；Rust 正常硬件路径交给 GPU |
| `0x00f0-0x00ff` | 声道、Sound2、暂停/恢复/音量（`src/vm.rs:2408-2409`）；`0x00fc` 进入 `sound_call` 后按默认分支无操作 | `gi_dispa.c:271-290` + Qt/SDL sound backend | `sound_load_hint(0x00fc)` 本来就是可选预加载提示，无操作是合法策略；能力值由设备/编译后端决定，两边都可能合法返回无声音 |
| `0x0100-0x0103` | 超链接设置、请求和取消（`src/vm.rs:2264-2287`） | `gi_dispa.c:291-296` + `event.cpp`/window 绘制 | Rust 保存文本/图形命中区域；GarglK 写入像素 hyperlink mask |
| `0x0120-0x0151` | Unicode、UnicodeNorm、Unicode I/O、行回显/终止符（`src/vm.rs:2297-2305`、`2410-2443`） | `gi_dispa.c:297-327` | Rust 把数组直接写 VM 内存；Git/GarglK 通过 prototype 做数组暂存/回写 |
| `0x0160-0x016f` | 日期时间（`src/vm.rs:2409-2410`） | `gi_dispa.c:328-339` + `cgdate.cpp` | 行为由各自日期实现决定，接口覆盖一致 |

GarglK 还编译了 `GLK_MODULE_GARGLKTEXT` 的 `0x1100-0x1103`（Z-code 颜色和反色扩展，`cheapglk/gi_dispa.c:340-345`），以及不通过标准 `@glk` selector 暴露的文件资源、像素窗口尺寸、overlay、`garglk_unput_string` 和 `zbleep` 等函数（`cheapglk/glk.h:504-633`）。当前 Rust 对这些扩展没有 API；Git 的 `glkop.c` 也只负责把 selector 转给所链接的 Glk，不会自动提供 GarglK 扩展。它们属于宿主/厂商扩展，不是 Glk 0.7.6 的标准缺口。

Rust 当前把 `GraphicsCharInput`（Glk selector 23）报告为不支持（`src/vm.rs:2517-2519`）；GarglK 的 `cggestal.cpp:121-127` 同样返回 false。Rust 的标准 Glk selector 范围基本齐全，但“完整支持”仍需按 host 能力、参数边界和实际媒体后端分别验证。

### 对当前性能的意义

Rust 没有 Git 那种通用 prototype marshalling，所以少了指针/数组临时转换层；但它在 `Vm::glk` 的每次调用创建参数 `Vec`，而 Git 对高频单字符路径有专用分支。优先优化批量输出、`put_char`、`stream_set_current`、`select_poll` 等热 selector，使用固定小数组或栈上存储，并把逐字符的呈现修订合并到一次输出边界。不要把 C 的对象指针注册表搬进 Rust；Rust 的整数句柄和序列化边界更适合当前架构。

## 四、事件、输入与线程

### `glulx-rs`

Rust 通过 `pending_select`、请求 map 和事件队列表达等待状态。`select_event` 保存事件地址和结果目的地，设置 `WaitingForEvent`，若已有输入请求则选择第一个窗口（`src/vm/events.rs:293-313`）。`poll_events` 处理音频、定时器，然后把队首事件写入 VM 内存并恢复运行（`src/vm/events.rs:228-246`）。`select_poll` 只取 Arrange/Redraw/SoundNotify/Timer 类型（`src/vm/events.rs:207-220`），与 Glk 的“只返回内部事件”规则一致。

图形播放器在 `story_view` 中从 egui 事件得到点击位置，调用 `mouse_input`/`hyperlink_input`（`src/app.rs:1262-1300`）；字符输入从 egui 事件中取一个键，把粘贴的其余字符放入 `pending_keys`（`src/app.rs:1316-1354`）。行输入提交会先把 UI 输入写回 VM，再调用 `provide_input`（`src/app.rs:1468-1513`、`964-987`）。这些调用发生在 UI 线程；下一次 `logic` 才进入 `run_vm`（`src/app.rs:1757-1765`）。

当前应用显式创建的后台线程承担翻译、图片解码、采样音频准备、故事加载和诊断等宿主辅助工作；音频库可能另有自己的设备线程。VM、Glk 状态、窗口视图和 eframe Context 没有共享所有权通道。worker 只返回带请求 ID 的结果，Glk 状态仍由 VM owner 串行更新。

### GarglK + Git

GarglK 的 `gli_events` 是一个事件 list；只有 SDL 音频回调跨线程时才用 mutex（`garglk/event.cpp:27-38`）。`gli_event_store` 追加事件，`gli_dispatch_event` 在普通 select 取队首，在 `select_poll` 中只寻找 Arrange/Redraw/SoundNotify/Timer（`garglk/event.cpp:51-97`）。

`glk_select` 先处理粘贴缓冲，再进入宿主的 `gli_select`（`garglk/event.cpp:209-235`）。Qt 路径在 `gli_select` 中清空事件、调用 `app->processEvents()`、刷新像素缓冲；没有事件时调用 `QEventLoop::WaitForMoreEvents` 阻塞等待，再继续 dispatch（`garglk/sysqt.cpp:1060-1090`）。键盘事件由 Qt `keyPressEvent` 直接转成 Glk keycode；鼠标事件也直接调用 `gli_input_handle_click`（`garglk/sysqt.cpp:546-703`、`747-778`）。因此 GarglK 的 input/select 是同步、事件泵驱动模型。

GarglK 的 launcher 与解释器进程边界在 `launchqt.cpp:127-172`：启动器通过 `QProcess` 启动 Git，并等待其结束。解释器进程自身由 `garglk/main.c:26-40` 调用 `garglk_startup`、`glk_main` 和 `glk_exit`，没有把 VM 执行再拆到后台线程。

### 三者差异如何解释“点击后卡住”

- Rust 的点击先完成 egui UI 构建，再在同一 UI 回调末尾向 VM 投递；如果上一个 `run_vm_slice`、`publish_story` 或同步媒体操作尚未结束，点击只能等线程返回。
- Rust 的图片和采样音频解码已经在 worker 线程执行；UI 线程只消费结果、创建纹理和提交绘制命令。SONG 需要跨资源组装，仍走 VM 内的同步路径。故事文件读取和 VM 建立也由 loader worker 承担。
- GarglK 遇到新图像或声音时也在解释器线程同步加载，但其等待输入时由 Qt 事件循环接管；Rust 是固定周期 eframe 帧循环。二者的等待策略不同，不能把“CPU 没吃满”当成某个线程没有运行的证据。

## 五、文本、字体和图形渲染

### 文本与字体

Rust 的文本缓冲区把 VM 的 `TextRun` 转成 Token，再为每个词/空格/长词片段创建 egui `LayoutJob`/`Galley`（`src/app/text_buffer.rs:366-406`、`414-532`）。布局缓存按内容修订、宽度、字号、超链接颜色和像素比例缓存（`src/app/text_buffer.rs:535-587`），只有可见 item 才绘制，但缓存失效时仍需重新测量和排版。网格按字符/样式/链接键缓存 galley，并对非空白单元绘制（`src/app/text_grid.rs:22-78`、`138-165`）。

样式中的斜体由 `ResolvedStyle.oblique` 映射到 `RichText::italics()`，比例/等宽由两个 egui family 选择（`src/app/text_buffer.rs:366-384`、`src/app/fonts.rs:160-183`）。加粗是在原 galley 旁边以约 0.45 像素偏移再次绘制（`src/app/text_buffer.rs:598-612`；网格为 `src/app/text_grid.rs:159-164`）。这套实现依赖 egui 的 shaping 与字体回退，斜体不一定来自与普通字形同一字体文件。

GarglK 的 `FontFace` 明确表示比例/等宽、普通/粗体、普通/斜体八种组合（`garglk/garglk.h:70-94`）。FreeType 加载时可以选真实 Bold/Italic/BoldItalic 文件，也可以对轮廓应用 embolden 和 oblique transform（`garglk/draw.cpp:498-539`、`274-369`）；字形和 kerning 结果放入缓存，缺字再查 substitution font，最后才替换问号（`garglk/draw.cpp:791-923`）。默认 Gargoyle 字体和每个 style face 在启动时建立（`garglk/draw.cpp:639-677`）。这解释了为什么参考图中的斜体、字距和换行不能由 Rust 仅复用字体名称保证一致。

### 文本缓冲区与网格

GarglK 的文本缓冲区按 `tbline_t` 保存滚屏行、属性、边缘图片和 flow break，默认 `SCROLLBACK=512`、每行 `TBLINELEN=300`（`garglk/garglk.h:489-499`、`1022-1100`）。重排只在窗口可用像素宽度变化时发生，并从行缓冲重建文本（`garglk/wintext.cpp:115-246`、`254-302`）。重绘时跳过未 dirty 的行（`garglk/wintext.cpp:460-507`），然后按连续属性运行绘制背景、链接和字形（`garglk/wintext.cpp:632-731`）。

GarglK 网格同样按行 dirty，连续相同属性的单元合并背景和链接处理（`garglk/wingrid.cpp:76-131`）。Rust 的 `WindowView` 仍以整个窗口的 `runs`/`grid_cells` 对外发布；虽然最近增加了按窗口 `content_revision` 的增量发布（`src/app.rs:848-943`），窗口内容改变后仍需复制变化窗口的完整向量并由 egui 绘制可见单元。

### 图形窗口

Rust 的 `Canvas` 保留 Fill/Image 操作，正常路径交给 egui/OpenGL；软件 OpenGL 或会话保存才建立 CPU 像素/纹理（`src/app/canvas.rs:1-31`、`95-117`、`309-390`）。操作数超过 1024 时把画布栅格化并重新建立一张图（`src/app/canvas.rs:290-307`）。图片命中区域独立保存，绘制和超链接输入使用同一 clip（`src/app/canvas.rs:238-288`）。

GarglK 的全局 `gli_image_rgb` 是 CPU RGB 画布；`win_graphics_redraw` 在 dirty 或强制重绘时逐像素复制图形窗口（`garglk/wingfx.cpp:75-96`），Qt 只在刷新时把这块物理像素缓冲作为 `QImage` 画出（`garglk/sysqt.cpp:407-431`）。它的路径更接近传统即时 rasterizer，Rust 的路径更接近 retained GPU scene。Rust 在硬件 OpenGL 下通常更适合大图缩放，但操作历史压缩、软件路径和会话保存会回到 CPU。

## 六、故事与资源加载

Rust 的 `Story::open_with_resources` 对文件执行 `std::fs::read`，`from_bytes` 对 Blorb 复制 container、提取 GLUL 并再截断到 `EXTSTART`（`src/story.rs:172-227`）。`Story` 同时保留 `image`、可选 `container`、外部资源内容和资源偏移表（`src/story.rs:146-170`）；这使会话可以脱离原文件恢复，但启动时存在多份字节副本。故事加载现在由 loader worker 承担；图片首用时由 decode worker 准备，声音播放时再由 audio worker 建立采样或 tracker 源。

Git 的 `git()` API 接受调用方提供的内存指针，`initMemory` 不复制 ROM；`gitWithStream()` 才为流读取分配完整游戏缓冲（`terps/git/git.c:106-174`）。Git README 明确建议 OS 支持时使用 mmap；Windows Git 用 `CreateFileMapping`/`MapViewOfFile` 后直接调用 `git`（`terps/git/git_windows.c:75-105`）。Blorb 由 `giblorb_set_resource_map` 建立映射，并通过 `FilePos` 找 Exec chunk（`terps/git/git.c:68-103`）。

GarglK 的启动器先检测文件头/扩展名、为 Blorb 查找 Exec，再以 `QProcess` 启动具体解释器（`garglk/launcher.cpp:219-283`、`336-375`）。图片由 `gli_picture_load` 从 Blorb 或 `PIC<number>` 读入，解码后按原图/缩放图放进 `picstore`；引用计数归零才清空（`garglk/imgload.cpp:35-93`、`95-160`）。声音路径把资源复制到 channel 内存，再交给 Qt、SDL 或其他后端播放（`garglk/sndsdl3.cpp:396-452`、`garglk/sndsdl2.cpp:562-620`）。

因此资源策略是：Rust 更重视统一校验、外部资源挂载和可序列化会话；Git 更重视零拷贝/mmap 和解释器启动速度；GarglK 依靠 Blorb map、每类媒体的宿主缓存和独立解释器进程。

## 七、标准兼容性与专有扩展

### Glulx

Rust 只接受 `0x00020000..=0x000301ff` 的故事版本（`src/story.rs:53-55`），支持 1/2/4 字节局部变量（`src/vm.rs:1598-1617`）和 Glulx 3.1.3 双精度/扩展 undo。Git 显式接受 1.0、2.0、3.0 和 3.1（`terps/git/git.c:30-57`），但 README 记录不支持旧 Superglus 的 1/2 字节局部变量，direct search key 也必须正好 4 字节。版本覆盖和限制是两者的兼容性差异，不能把 Git 的速度路径当作 Rust 的行为合同。

两者都实现 Glulx 标准 gestalt 0–13；Git `gestalt.c` 额外报告 Git cache control（`terps/git/gestalt.c:3-59`），Rust 未实现该 Git 专有 selector，只在标准 selector 上报告 3.1.3、浮点、双精度、扩展 undo 和 acceleration（`src/vm.rs:1907-1927`）。`git_setcacheram=0x7940` 与 `git_prunecache=0x7941` 在 `terps/git/terp.c:1734-1742`，不是 Glulx 标准缺口；若未来实现 decoded cache，可以把它们作为兼容扩展单独设计。

### Glk

Rust `glk_gestalt` 返回 Glk 0.7.6，并按当前 host 的图形、终端、音频、字体和窗口类型报告能力（`src/vm.rs:2479-2524`）。图形播放器声明图形、鼠标、链接、图片缩放；终端 host 则关闭图形/音频并提供文本窗口。未知 selector 返回零并记录，不会伪造宿主能力。

GarglK 的 `cggestal.cpp` 也报告 Glk 0.7.6，图片、声音、Unicode、日期、链接和行终止符等能力由编译模块/配置决定（`garglk/cheapglk/cggestal.cpp:44-210`）。其 `gestalt_GarglkText=0x1100`、overlay 等接口是 GarglK 扩展，不能要求 Rust 的标准 Glk dispatcher 提供相同 selector。Git 只通过所链接的 Glk 库得到这些能力；`glkop.c` 本身并不决定 GUI 是否有声音或图片。

### 存档边界

Rust 的可移植 IFZS 使用 `CMem`/`Stks`/`MAll` 并在替换状态前验证数据（`src/vm/save.rs:19-60`、`63-180`）；桌面会话另行序列化整个 VM、文本和画布（`src/app.rs:1699-1750`）。Git 的 `savefile.c`/`saveundo.c` 以原始栈、内存状态和 heap summary 工作；GarglK 宿主只负责文件/对话框。这里 Rust 的宿主快照范围更大，不能与 Git 的可移植存档大小直接比较。

## 八、建议的优化顺序

1. **先补可观测性。** 将一次点击到画面更新拆为 `input received -> VM resumed -> VM boundary -> window view -> layout -> texture/audio -> paint`，给每段记录耗时和线程。现有 `diagnostics::stage` 和 `vm` 心跳可作为入口，但需要把 `publish_story`、文本布局、图片解码、音频准备和 undo 单独计时。
2. **去掉 UI 线程上的大块同步工作。** 图片/音频解码、故事/资源读取、会话编码可以在线程中完成；结果通过有序消息回到 UI，VM 仍由一个 owner 执行 Glk 状态变更。不要让 worker 直接访问 `egui::Context` 或 `Vm` 的可变状态。
3. **实现 Rust decoded-block cache。** 先缓存安全的中间表示和控制流，命中后跳过模式字节/立即数解码；以写入 RAM 的地址范围和 `setmemsize` 为失效依据。之后再评估是否值得做更激进的 native/JIT 后端。
4. **把 undo 改为页级差分或 copy-on-write。** 256 字节页与 Glulx 的内存对齐一致；快照只保留改变页、页表、栈和 heap 元数据，预算按实际拥有页计费。恢复时保留 protection 覆盖的当前内容，继续遵循现有 IFZS/undo 状态边界。
5. **减少热 Glk 分配和呈现复制。** 为最多八个参数使用栈上固定数组；输出批量追加到 run/event buffer，边界时一次增加 content revision；窗口视图可以继续按变化窗口发布，不要退回每帧复制整个 window tree。
6. **针对字体和排版做差分验收。** 以同一个字体文件、字号、DPI 和窗口宽度，在 Rust 与 GarglK 对比普通、粗体、斜体、粗斜体、CJK 回退、kerning、段落缩进、全行对齐和边缘图像。GarglK 的真实 italic face/FreeType transform 是参考行为，egui 的 galley 只作为 Rust 的实现细节。

## 复核范围与限制

本报告使用三个上游仓库的一手源码作为证据，没有声称静态差异或单次 benchmark 能代表所有游戏。标准功能是否“支持”还要结合实际 Glk 模块编译选项和宿主设备，不能只由 dispatcher 表推断。当前结论最适合用来决定 profiling 点和优化顺序，不能替代 Windows/macOS 实机输入、字体和图形回归。

## 一手来源索引

- Rust VM：[vm.rs](https://github.com/KagurazakaNyaa/glulx-rs/blob/main/src/vm.rs)、[events.rs](https://github.com/KagurazakaNyaa/glulx-rs/blob/main/src/vm/events.rs)、[presentation.rs](https://github.com/KagurazakaNyaa/glulx-rs/blob/main/src/vm/presentation.rs)、[memory.rs](https://github.com/KagurazakaNyaa/glulx-rs/blob/main/src/memory.rs)、[save.rs](https://github.com/KagurazakaNyaa/glulx-rs/blob/main/src/vm/save.rs)
- Rust 播放器：[app.rs](https://github.com/KagurazakaNyaa/glulx-rs/blob/main/src/app.rs)、[text_buffer.rs](https://github.com/KagurazakaNyaa/glulx-rs/blob/main/src/app/text_buffer.rs)、[text_grid.rs](https://github.com/KagurazakaNyaa/glulx-rs/blob/main/src/app/text_grid.rs)、[canvas.rs](https://github.com/KagurazakaNyaa/glulx-rs/blob/main/src/app/canvas.rs)、[fonts.rs](https://github.com/KagurazakaNyaa/glulx-rs/blob/main/src/app/fonts.rs)、[story.rs](https://github.com/KagurazakaNyaa/glulx-rs/blob/main/src/story.rs)、[sound.rs](https://github.com/KagurazakaNyaa/glulx-rs/blob/main/src/vm/sound.rs)
- Git 1.3.8：[README.txt](https://github.com/DavidKinder/Git/blob/master/README.txt)、[terp.c](https://github.com/DavidKinder/Git/blob/master/terp.c)、[compiler.c](https://github.com/DavidKinder/Git/blob/master/compiler.c)、[compiler.h](https://github.com/DavidKinder/Git/blob/master/compiler.h)、[peephole.c](https://github.com/DavidKinder/Git/blob/master/peephole.c)、[memory.c](https://github.com/DavidKinder/Git/blob/master/memory.c)、[memory.h](https://github.com/DavidKinder/Git/blob/master/memory.h)、[saveundo.c](https://github.com/DavidKinder/Git/blob/master/saveundo.c)、[glkop.c](https://github.com/DavidKinder/Git/blob/master/glkop.c)、[git.c](https://github.com/DavidKinder/Git/blob/master/git.c)、[git_windows.c](https://github.com/DavidKinder/Git/blob/master/git_windows.c)、[gestalt.c](https://github.com/DavidKinder/Git/blob/master/gestalt.c)
- GarglK：[launcher.cpp](https://github.com/garglk/garglk/blob/master/garglk/launcher.cpp)、[main.c](https://github.com/garglk/garglk/blob/master/garglk/main.c)、[sysqt.cpp](https://github.com/garglk/garglk/blob/master/garglk/sysqt.cpp)、[event.cpp](https://github.com/garglk/garglk/blob/master/garglk/event.cpp)、[window.cpp](https://github.com/garglk/garglk/blob/master/garglk/window.cpp)、[wintext.cpp](https://github.com/garglk/garglk/blob/master/garglk/wintext.cpp)、[wingrid.cpp](https://github.com/garglk/garglk/blob/master/garglk/wingrid.cpp)、[wingfx.cpp](https://github.com/garglk/garglk/blob/master/garglk/wingfx.cpp)、[draw.cpp](https://github.com/garglk/garglk/blob/master/garglk/draw.cpp)、[imgload.cpp](https://github.com/garglk/garglk/blob/master/garglk/imgload.cpp)、[gi_dispa.c](https://github.com/garglk/garglk/blob/master/garglk/cheapglk/gi_dispa.c)、[cggestal.cpp](https://github.com/garglk/garglk/blob/master/garglk/cheapglk/cggestal.cpp)
