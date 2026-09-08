# Glulx 实现验收记录

日期：2026-09-08，Linux x86_64，核心实现提交 `4c16443`，加速与媒体提交 `c5fcc20`。本轮进一步修复完整规范审计发现的核心、窗口、输入、流、字体及呈现边界。

## 可复现命令与结果

```sh
RUSTC_WRAPPER= cargo fmt --all -- --check
RUSTC_WRAPPER= cargo test --all-targets
RUSTC_WRAPPER= cargo clippy --all-targets -- -D warnings
RUSTC_WRAPPER= cargo build --release
python3 tools/check-opcodes.py --spec /path/to/Glulx-Spec.md --output /tmp/glulx-opcode-audit.tsv
python3 tools/check-reference.py --reference /path/to/glulxe --candidate target/debug/glulx-rs --fixtures /path/to/fixtures
```

Rust 测试 136 passed / 0 failed；Clippy（warnings 视为错误）及 release 构建通过。
脚本不下载样本、不修改仓库游戏资源；合成故事及存档使用临时目录。
不传 `--fixtures` 仍可运行合成 IFZS 双向互操作、double stack 顺序、Inform 加速函数、零长度内存及深层字符串差分检查。
官方 opcode 表 150/150 条分发和操作数数量匹配；官方 Glk dispatch 注册表 124/124 selectors 均有分发。这两项只证明表完整，执行语义仍需运行测试。

此前完整脚本通过结果：

```text
PASS Rust -> Glulxe: save continuation, heap chunk, double stack order
PASS Glulxe -> Rust: save continuation, heap chunk, double stack order
PASS acceleration: all 13 functions, 94 result checks, exact reference transcript
PASS core boundaries: zero-length memory operations and 40000 Huffman substrings, exact reference transcript
PASS glulxercise.ulx: 92 passing sections
PASS unicasetest.ulx: exact normalized reference transcript
PASS resstreamtest.gblorb: exact normalized reference transcript
PASS Adventure Rust -> Glulxe
PASS Adventure Glulxe -> Rust
```

Glulxercise 输入 `all / allfloat / alldouble / quit`，三次 `All tests passed.`。
其随机分布测试有统计性误报概率，官方样本也明确说明；单次统计失败应记录并分析，不能用重复运行掩盖确定性缺陷。本轮提交前重跑，random 组 240 次取样出现 lobit=141 / hibit=99，超出样本设置的 [100..140]，导致该组两个断言失败；其他组通过，allfloat/alldouble 全通过。此结果记录为统计阈值失败，不算本次整套通过，亦未靠重跑隐藏它；此前完整通过结果保留为历史证据。
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
| IFZS | round trip、损坏存档、文件提示、空 MAll | 合成故事及 Adventure 双向互读 |
| undo/restart/protect/heap | `undo_*`、`allocation_limits_*`、原 heap 回归 | Glulxercise 综合 |
| Inform 加速 1–13 | [acceleration.rs](../src/vm/acceleration.rs)：注册/取消、类/属性/私有权限、旧/新布局、call/callf/tailcall、压缩字符串和 20,000 次 filter 回调、恢复状态边界 | 13 函数共 94 项结果与 Glulxe 精确一致 |
| random/verify/gestalt | `random_ranges_determinism_verify_and_capabilities` | Glulxercise 综合 |
| 流/dispatch | read/seek/Unicode/count、echo cycles、流关闭解除绑定、写入末尾定位、UTF-8 字节标记和覆盖、旧会话迁移、原栈引用测试 | resstreamtest 与参考完全一致 |
| 窗口/事件 | 排列方向/嵌套 key/关闭/resize/字体度量、多窗口输入、取消/计时器、select_poll 事件分类、图形裁剪和背景扩展 | twocol 启动及 Sensory GUI |
| Unicode | 扩展转换、titlecase、NFC/NFD、能力参数 | unicasetest 与参考完全一致 |
| 文本图像 | [presentation.rs](../src/vm/presentation.rs) 图片顺序、事件关联、动态尺寸、零尺寸及会话；[text_buffer.rs](../src/app/text_buffer.rs) 行内基线、双侧/重复边栏、flow-break、换行/单词、缩放与裁剪 | 合成故事 GUI 缩放/点击/恢复 |
| 样式/鼠标/链接/终止键 | 样式快照及真实测量、缩进/四种对齐、echo 样式传播、固定网格样式/链接/编辑；真实中文 glyph 绘制及缺字能力 | 25 种特殊按键、网格 LINK/预填编辑、官方定时取消和恢复编辑 GUI 验收 |
| 日期/时间 | epoch、负时间、字段规范化、失败 sentinel、UTC 往返、New York DST 间隙及 Apia 跳日 | datetimetest 启动 |
| 声音 | 无设备 idle sink：同步采样起播、独立暂停/音量、停止/结束/渐变通知；[tracker.rs](../src/vm/sound/tracker.rs)：PCM、速度/BPM/音量/E6 循环、重复及恢复偏移 | Sensory AIFF；合成 MOD GUI 播放及完成通知 |
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


## 新增合成媒体验收

以下 fixture 完全由仓库脚本生成，包含原创 PNG 和四声道 MOD，不依赖下载游戏：

```sh
python3 tools/make-media-fixture.py /tmp/glulx-media.gblorb
cargo run -- /tmp/glulx-media.gblorb
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
python3 tools/make-style-fixture.py /tmp/glulx-styles.ulx
cargo run -- /tmp/glulx-styles.ulx
python3 tools/check-input-ui.py --candidate target/debug/glulx-rs --output /tmp/glulx-input-ui --input-feature /path/to/inputfeaturetest.ulx
```

样式故事经 Linux Xvfb 验收：点击网格 LINK，预填 Ada 改为 Grace Hopper 并提交，正常关闭重开后保留事件、文本和网格。Rust 绘制测试还验证实际中文 glyph、宽字形单格压缩、斜体/字重/颜色、网格链接命中和编辑。

输入脚本自动分配 X display，保留截图、应用日志及持久会话；使用正常 WM_DELETE_WINDOW 退出。原创故事检查 25 个原生按键事件（F1–F12、方向键、Delete/Backspace、Esc/Tab/Page/Home/End/Enter）的精确值，定时取消时组成的 abc，以及同一 VM 执行片段重新请求时的 NEW 预填。官方 Input Feature Test 检查定时取消后的 ROT13 显示、保留原文 abcdef，并在会话恢复后继续编辑。

图形回归验证：改变尺寸立即保留左上可见像素，裁去缩小区域，用当前背景填充新增区域；缩小后再放大不会恢复已裁像素，零尺寸释放画布。无符号矩形宽高按规范裁剪，包含 0xFFFFFFFF 和负坐标组合。
