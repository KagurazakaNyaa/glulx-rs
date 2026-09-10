# Glulx 实现验收记录

[English](glulx-validation.md) | [中文](glulx-validation.ZH.md)

日期：2026-09-08，Linux x86_64，核心实现提交 `4c16443`，加速与媒体提交 `c5fcc20`，输入/字体/窗口提交 `4870bcf`。随后继续完成媒体格式、共享文件、宽范围日期、IFZS 及图片边界修复。

缺口复核文档已先提交为 `0a6d5b4`，双语文档独立提交为 `af210ba`。随后实现的独立资源、SONG、交互终端和真实 light 字重已有下述本地验收；尚未执行的平台及更广场景继续保留在 [规范清单](glulx-spec-checklist.ZH.md) 中。

## 可复现命令与结果

```sh
RUSTC_WRAPPER= cargo fmt --all -- --check
RUSTC_WRAPPER= cargo test --all-targets
RUSTC_WRAPPER= cargo clippy --all-targets -- -D warnings
RUSTC_WRAPPER= cargo build --release
python3 tools/check-opcodes.py --spec "<spec-path>/Glulx-Spec.md" --output "<output-dir>/glulx-opcode-audit.tsv"
python3 tools/check-reference.py --reference "<glulxe-path>" --candidate target/debug/glulx-rs --fixtures "<fixtures-dir>"
```

Rust 测试 181 passed / 0 failed（179 个库测试、2 个 CLI 测试）；Clippy（warnings 视为错误）及 release 构建通过。
脚本不下载样本、不修改仓库游戏资源；合成故事及存档使用临时目录。
不传 `--fixtures` 仍可运行合成 IFZS 双向互操作、double stack 顺序、Inform 加速函数、零长度内存及深层字符串差分检查。
官方 opcode 表 150/150 条分发和操作数数量匹配；官方 Glk dispatch 注册表 124/124 selectors 均有分发。这两项只证明表完整，执行语义仍需运行测试。

最终整合后的完整脚本通过结果：

```text
PASS Rust -> Glulxe: save continuation, heap chunk, double stack order
PASS Glulxe -> Rust: save continuation, heap chunk, double stack order
PASS acceleration: all 13 functions, 94 result checks, exact reference transcript
PASS core boundaries: zero-length memory operations and 40000 Huffman substrings, exact reference transcript
PASS shared file streams: cross-handle reads, independent counts, and final file bytes match reference
PASS glulxercise.ulx: 92 passing sections
PASS unicasetest.ulx: exact normalized reference transcript
PASS resstreamtest.gblorb: exact normalized reference transcript
PASS Adventure Rust -> Glulxe
PASS Adventure Glulxe -> Rust
```

Glulxercise 输入 `all / allfloat / alldouble / quit`，三次 `All tests passed.`。
其随机分布测试有统计性误报概率，官方样本也明确说明；单次统计失败应记录并分析，不能用重复运行掩盖确定性缺陷。`4870bcf` 提交前的一次重跑，random 组 240 次取样出现 lobit=141 / hibit=99，超出样本设置的 [100..140]，导致该组两个断言失败；其他组通过，allfloat/alldouble 全通过。该次记录为统计阈值失败，不算整套通过。随后共享流和 IFZS 改动完成后的必要整合回归，92 个段落及三轮全部通过；此处保留前次统计失败，未修改随机实现或样本阈值。脚本现在会保留失败全文并给出日志路径。
文件流 UTF-8 byte mark/seek 按规范测试；CheapGlk 的 Unicode text stream 存在 mark 除以 4 的实现差异，该边界不宣称差分一致。
Unicode 输入 `all / quit`，资源流输入 `quit`；比较只归一化 interpreter version 字段，Glulxe 使用 `-q -u` 保证 UTF-8。
Adventure 的保存方执行 `north / save / 路径 / quit / y`，另一解释器执行
`restore / 路径 / look / quit / y`，验证恢复到 `In Forest`，双向均通过。

## 测试矩阵

本地主要证据在 [conformance.rs](../src/vm/conformance.rs)，原有 VM、Memory、Story 和 GUI 测试仍保留。

| 清单领域 | 本地回归入口/覆盖 | 外部验证 |
| --- | --- | --- |
| 解码、寻址、栈、局部变量 | `all_load_address_modes_and_opcode_encodings`、`narrow_copy_integer_extremes_and_stack_bounds`、零长度内存操作、locals 帧容量 | Glulxercise 综合 |
| 调用、搜索、字符串/filter | 原 VM 测试、`huffman_leaf_and_indirection_matrix`、迭代输出续体及数值 filter 内存档 | Glulxercise 综合；40,000 次 Huffman 子字符串与参考一致 |
| 单/双精度 | `double_*`、`floating_branches_*`、`float_nan_modulo_and_power_identities` | Glulxercise allfloat/alldouble；双栈结果参考测试 |
| IFZS | round trip、损坏存档、文件提示、空 MAll、重复注释/扩展/单例块 | 合成故事及 Adventure 双向互读 |
| undo/restart/protect/heap | `undo_*`、`allocation_limits_*`、原 heap 回归 | Glulxercise 综合 |
| Inform 加速 1–13 | [acceleration.rs](../src/vm/acceleration.rs)：注册/取消、类/属性/私有权限、旧/新布局、call/callf/tailcall、压缩字符串和 20,000 次 filter 回调、恢复状态边界 | 13 函数共 94 项结果与 Glulxe 精确一致 |
| random/verify/gestalt | `random_ranges_determinism_verify_and_capabilities` | Glulxercise 综合 |
| 流/dispatch | read/seek/Unicode/count、echo cycles、流关闭解除绑定、同文件共享缓存/独立位置/dirty flush/旧会话迁移、写入末尾定位、UTF-8 字节标记和覆盖、旧会话迁移、原栈引用测试 | resstreamtest 与参考完全一致；共享流输出/计数/最终文件字节差分一致 |
| 窗口/事件 | 排列方向/嵌套 key/关闭/resize/字体度量、多窗口输入、取消/计时器、select_poll 事件分类、图形裁剪和背景扩展 | twocol 启动及 Sensory GUI |
| Unicode | 扩展转换、titlecase、NFC/NFD、能力参数 | unicasetest 与参考完全一致 |
| 文本图像 | [presentation.rs](../src/vm/presentation.rs) 图片顺序、事件关联、动态尺寸、零尺寸及会话；[text_buffer.rs](../src/app/text_buffer.rs) 行内基线、双侧/重复边栏、flow-break、换行/单词、缩放与裁剪 | 合成故事 GUI 缩放/点击/恢复 |
| 样式/鼠标/链接/终止键 | 样式快照及真实测量、缩进/四种对齐、echo 样式传播、固定网格样式/链接/编辑；真实中文 glyph 绘制及缺字能力 | 25 种特殊按键、网格 LINK/预填编辑、官方定时取消和恢复编辑 GUI 验收 |
| 日期/时间 | epoch、负时间、完整 i32 年份、字段/负微秒规范化、UTC 往返、New York DST 间隙、Apia 跳日及远古/未来偏移 | datetimetest 启动 |
| 声音 | 无设备 idle sink：同步采样起播、独立暂停/音量、停止/结束/渐变通知；[tracker.rs](../src/vm/sound/tracker.rs)：四格式 PCM、S3M OPL、速度/BPM/音量/E6 循环、重复及恢复偏移；sampled 流式编码/时长/重采样边界 | Sensory AIFF；合成 MOD GUI 播放及完成通知 |
| Blorb/产品 | 容器边界/资源索引/元数据测试、session serialization | Adventure/Sensory 关闭重开恢复 |

矩阵是按领域覆盖，不意味着每个合法/非法输入组合均已穷举。

## 参考版本与样本

Glulxe revision `56ab8743bab565de307bd892c555d8d8897ed517`；CheapGlk revision
`14d8aaf6e4150669762bd4646a5368e75c1eeee6`。分别来自
[Glulxe](https://github.com/erkyrath/glulxe) 和 [CheapGlk](https://github.com/erkyrath/cheapglk)。
在相邻目录构建 CheapGlk 后构建 Glulxe；本轮使用 `OPTIONS='-O2 -Wall -DOS_UNIX -DUNIX_RAND_GETRANDOM'`。
CheapGlk 不提供桌面图形/声音，因此这些能力不按其返回值盲目对齐。

除 Adventure 外均来自 [作者官方 Glulx 样本页](https://eblong.com/zarf/glulx/)，下载地址为该目录加下表文件名。
Adventure 下载自 [IF Archive](https://www.ifarchive.org/if-archive/games/glulx/advent.ulx)，在 fixture 目录命名为 `glulx-advent.ulx`。
样本和参考解释器源码不随本仓库分发。

| 文件 | SHA-256 |
| --- | --- |
| glulxercise.ulx | `b732127fee4cb266a5330981c1111fdfaba237134525754e063e6dc5f449b348` |
| unicasetest.ulx | `e4b2da7fe1a894913421ba87cf26551f18fa158294df2333bbb79bc39b2f219c` |
| resstreamtest.gblorb | `1d7c77d830913447e08594670c2d8ee03df75517bf46a5d2b70607422777837f` |
| glulx-advent.ulx | `264236f2c3504eb2f326ab560aef435902d357c1ef848127b6ea0329c10387f8` |
| sensory.blb | `a05cd29a71b3200e564a1f33146200f88664aab394b9924acab99381a4001afd` |
| inputfeaturetest.ulx | `1fe2d4c126dd883abfc0f19c41676d34973f424c58f00b4ab5a0ee24c348552c` |
| datetimetest.ulx | `b32ec0803c60a31de07c4c23c00bb5d4f8dbe258956392813b8af0847d71a0b5` |
| twocol.ulx | `23b2acb6ba725236b9db010342f607a41e9fc5e44758a7360b12d102418b0e6d` |

Glulxercise 二进制为 Release 13 / 241202；同目录下载到的 `.inf` 是 Release 10 / 220722，不能混作同一版本。
Unicode 为 Release 3，资源流 Release 2，输入扩展 Release 1，日期时间 Release 4，Sensory Jam Release 4，Adventure Release 5 / 961209。

## 桌面验收与限制

Linux Xvfb、软件 OpenGL，通过 X11 聚焦窗口后发送输入，使用正常
`WM_DELETE_WINDOW` 退出，再不带故事参数启动：

- Adventure：`north`，关闭、重开，持久会话保留 `In Forest`。
- Sensory Jam：`hit gong / east / examine photograph`，AIFF 播放返回成功且无不支持提示，图片可见；关闭重开后文字、图形画布仍在。

声音验收确认设备/解码/播放路径及事件状态，未做人耳听音或采样波形比对。
输入扩展、日期、多窗口样本另通过 headless 启动、指令及退出冒烟；这不代表这些样本所有交互项均自动验证。
Windows/macOS 尚未运行本轮 GUI 测试。未完成任何长篇游戏全通关，也不声明所有媒体编码或实体声卡波形已验证。Sound2 的多声道同步已在软件输出层按立体声采样帧验证。

真实终端验收需使用 PTY/TTY 检查网格显示、预填编辑、定时取消和无回显单键输入；现有管道转录比较不作为该能力的通过证据。

后续验收需要分别提供 Windows/macOS 系统与构建版本、逐项操作结果及失败记录；完整游戏流程需要固定游戏版本和可复现路线；媒体变体需要记录编码参数、输出和失败行为。新增验收结果应注明平台与覆盖范围，再勾选对应待办。


## 新增合成媒体验收

以下 fixture 完全由仓库脚本生成，包含原创 PNG 和四声道 MOD，不依赖下载游戏：

```sh
python3 tools/make-media-fixture.py "<output-dir>/glulx-media.gblorb"
cargo run -- "<output-dir>/glulx-media.gblorb"
```

在 Linux Xvfb 中，窗口分别调整为 1100×820、700×820：三种行内图片对齐正确，
左右边栏按窗口宽度缩小，文字在图片旁绕排并在图片下恢复全宽。
图片点击产生 `Image hyperlink received.`，音频结束产生 `MOD playback completed.`；
正常关闭后不指定故事启动，这些文字、图像和待输入状态保留。再次运行 Sensory Jam
原有 AIFF/照片/恢复验收，确认媒体改动没有破坏既有路径。

合成故事忽略 Arrange 等非输入事件，以便窗口缩放不会错误结束测试。
零尺寸图片、无效 margin 放置、flow-break 失效、超宽图像和字体换行等边界由 Rust
测试覆盖。音量渐变的回归覆盖未及时轮询时中途替换与已完成渐变通知，保证从当前音量继续。


## 样式、输入、图形边界验收

原创样式 fixture 覆盖居中标题、悬挂缩进和两端对齐段落、右对齐、网格 LINK 及预填输入：

```sh
python3 tools/make-style-fixture.py "<output-dir>/glulx-styles.ulx"
cargo run -- "<output-dir>/glulx-styles.ulx"
python3 tools/check-input-ui.py --candidate target/debug/glulx-rs --output "<output-dir>/glulx-input-ui" --input-feature "<fixtures-dir>/inputfeaturetest.ulx"
```

样式故事经 Linux Xvfb 验收：点击网格 LINK，预填 Ada 改为 Grace Hopper 并提交，正常关闭重开后保留事件、文本和网格。Rust 绘制测试还验证实际中文 glyph、宽字形单格压缩、斜体/字重/颜色、网格链接命中和编辑。

输入脚本自动分配 X display，保留截图、应用日志及持久会话；使用正常 WM_DELETE_WINDOW 退出。原创故事检查 25 个原生按键事件（F1–F12、方向键、Delete/Backspace、Esc/Tab/Page/Home/End/Enter）的精确值，定时取消时组成的 abc，以及同一 VM 执行片段重新请求时的 NEW 预填。官方 Input Feature Test 检查定时取消后的 ROT13 显示、保留原文 abcdef，并在会话恢复后继续编辑。

图形回归验证：改变尺寸立即保留左上可见像素，裁去缩小区域，用当前背景填充新增区域；缩小后再放大不会恢复已裁像素，零尺寸释放画布。无符号矩形宽高按规范裁剪，包含 0xFFFFFFFF 和负坐标组合。


## 媒体格式与后续边界

```sh
python3 tools/check-graphics-ui.py --candidate target/debug/glulx-rs --output "<output-dir>/glulx-graphics-ui"
python3 tools/check-audio-codecs.py
```

图形脚本已通过：缩小窗口裁去右侧绿色方块，放大后绿色不再出现，左上红色方块保留且新增区域填充当前背景；绘制宽高 0xFFFFFFFF、负起点的图片成功，并保留到正常关闭/恢复之后。脚本直接检查截图像素及持久会话，而非仅确认进程未退出。Rust 回归另检查损坏 IDAT 返回失败、透明度合成，以及合法 20000×1 图片上传前适配 GPU 纹理边长，避免 debug panic；VM 仍保留原始图片尺寸。

Blorb RIdx 必须位于首块且唯一；RDes 解析验证 UTF-8、无条目间 padding、截断及重复条目，图像/声音文字描述显示于故事信息面板。依据 [Blorb 2.0.5](https://eblong.com/zarf/blorb/Blorb-Spec.md)。IFZS 重复块依据 [Quetzal 1.4 §8.8–8.9](https://www.ifarchive.org/if-archive/infocom/interpreters/specification/savefile_14.txt)。

日期原始字节码差分中，10^13 秒返回 YEAR:318857，与 Glulxe 一致。共享文件原始字节码检查不同句柄读取刚写入的内容、独立读写计数及最终文件 XYC；已并入 check-reference.py。


四种 tracker fixture 位于 [fixtures.rs](../src/vm/sound/tracker/fixtures.rs)，均为原创生成文件：MOD/XM/S3M/IT 验证非静音、结束、完整重复和帧对齐恢复；S3M 另检查 OPL 乐器的声音和音量衰减。采样源由 [sampled.rs](../src/vm/sound/sampled.rs) 逐包解码，保持编码数据共享，按容器整数帧数裁掉编码填充。播放重复按自然 EOF 计数，不依赖浮点 Duration。

音频 codec 工具用 ffmpeg 生成原创 44.1 kHz 立体声音调，再测试项目当前构建的解码器：AIFF/OGG/MP3 各 250 ms（22,050 个样本）和 1250 ms（110,250 个样本），共六组精确样本数、时长、有限/无限重复、5 ms 恢复及播放转换均通过。所用 Rodio/Symphonia 构建由 Cargo 的 JSON artifact 输出定位，避免选择旧 feature 组合。工具需要 cargo/rustc/ffmpeg；运行时播放器不依赖 ffmpeg。

18 项音频专项还验证 8/22.05/48 kHz 输入转 44.1 kHz 时逐样本等于完整缓冲区基准，解码分包及重复边界不重置重采样相位。


## 独立资源、终端输入、SONG 与真实 light 字重

```sh
python3 tools/check-resource-maps.py --fixture "<fixtures-dir>/resstreamtest.gblorb" --candidate target/debug/glulx-rs --reference "<glulxe-path>" --output "<output-dir>/resource-maps"
python3 tools/check-resource-ui.py --candidate target/debug/glulx-rs --output "<output-dir>/resource-ui"
python3 tools/check-terminal.py --candidate target/debug/glulx-rs
python3 tools/make-song-fixture.py "<output-dir>/glulx-song.gblorb"
python3 tools/check-song-ui.py --candidate target/debug/glulx-rs --output "<output-dir>/song-ui"
```

- 资源模型：新增 8 个 Rust 测试覆盖无执行文件的包、128 字节 IFhd、冲突、原子失败、发现优先级/歧义、散装类型和新旧桌面快照。官方资源流故事在原始包、raw 加显式包、自动发现和散装目录四种方式下逐字输出一致，并与归一化解释器版本后的 Glulxe 输出一致。
- 资源 GUI：CLI 选择、GUI 三种选项、Browse 包和 Use this directory、图片缓存切换、错误 IFhd 保留旧状态、资源路径输入焦点及切换故事无资源泄漏均通过。删除原包/目录和原始故事后，恢复会话仍能重新读取 Data 并重绘图片，验证的是资源内容恢复而非仅保存画布。
- TTY：真实 PTY 验收通过 25 个即时特殊键、预填编辑、定时取消取回当前组成、替换预填、网格编辑、多窗口选择、resize/Arrange、回显/终止键、文件写入/取消，以及正常退出、Ctrl+C/Ctrl+D 和 VM 错误后的终端恢复。管道输出仍精确匹配。已加入 Windows 控制台连接代码，但没有 Windows 实机验收。
- SONG：9 个专项覆盖 15/31 样本头、共享/完整 22 字节引用、AIFF 1–32 位 PCM、SSND offset、MARK/INST 无/正向/往返循环、损坏引用和边界、等价模块 PCM、重复、恢复偏移、暂停/停止及通知。原创桌面故事通过活动且暂停的会话恢复、继续播放/渐变完成、最后一次有限重复通知、双声道同步和停止。
- light 字重：元数据校验拒绝伪装成 light 的普通/损坏字体，实际安装的细字形通过命名字体族渲染，序列化后重新安装宿主可用性。Linux 桌面输出中文本缓冲区和网格的字重均为 -1，链接、原位编辑和会话恢复仍正常；缺少字体时保留普通回退。

参考回归仍通过全部 10 项，包含 Glulxercise 92 个段落及 Adventure 双向存档。资源、终端和 SONG 工具生成临时原创内容，不下载游戏。


## 依赖升级与发布准备

2026-09-08 通过 crates.io 官方 API 核对全部 21 个直接依赖，锁文件均解析为最新稳定版。Cargo.toml 保留兼容的主／小版本范围，Cargo.lock 记录实际验证集合。主要适配包括 eframe/egui 0.36.1、Rodio 0.22.2、Symphonia 0.6.1 和 reqwest 0.13.4。GUI 迁移新增回归，防止重复请求焦点打断输入法；音频保留精确编码长度和连续重采样测试，以连续 PCM 迭代器作为转换基准。

新版 reqwest 已用实际 HTTP/HTTPS 翻译适配器检查成功响应、认证头、限流、无效响应和缓存。发布 workflow 的就地注释记录最新稳定 Actions/工具链和最新 OS runner 的选择理由，以及 Ubuntu 22.04 构建 Linux 发行包的兼容性例外。

最终本地检查使用稳定 Rust 1.98.1；优化后的 release 可执行文件也通过终端套件和全部 10 项参考检查。


首次原生发布 CI 中，各平台 Rust 测试、Clippy、release 构建及 `--help` 启动检查通过。macOS 暴露了 PTY 验收脚本问题：会话首进程退出会撤销 slave，退出后 `tcgetattr` 返回 ENOTTY。脚本现用监督进程保留会话，直到完成终端属性、光标和屏幕恢复检查；Linux 套件通过，故意不恢复 raw mode 的程序仍被正确拒绝。macOS 修复确认继续由后续发布 CI 强制执行，没有跳过这些检查。


macOS runner 上的最小 Python `tty.setraw`/`tcsetattr` 往返在不运行播放器时也复现了剩余的属性精确比较失败：仅 PENDIN（0x20000000）改变。Apple XNU 在恢复 ICANON 时会设置该待处理输入状态位（[内核源码](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/tty.c)）。脚本仅归一化 Darwin 的 PENDIN，其他标志、速率和控制字符仍严格比较；ICANON/ECHO/ISIG/IEXTEN 变异及故意未恢复 raw mode 的真实程序均被拒绝。确认原因后删除临时诊断 workflow，完整 macOS 终端套件继续作为发布必需门槛。
