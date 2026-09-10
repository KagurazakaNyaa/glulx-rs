# Glulx 规范兼容性 checklist

[English](glulx-spec-checklist.md) | [中文](glulx-spec-checklist.ZH.md)

更新：2026-09-10。当前仓库状态为 `1a27dd4`。`[x]` 表示实现及列出的验证已完成，不表示穷尽规范认证；开放工作使用 `[ ]`。规范依据、类别和验收条件见 [剩余能力复核](glulx-remaining-spec-audit.ZH.md)。详细测试、样本版本、复现命令见 [验收记录](glulx-validation.ZH.md)，实现限制见 [兼容性](compatibility.ZH.md)。

## 规范边界

- [Glulx 3.1.3](https://eblong.com/zarf/glulx/Glulx-Spec.html)：VM、指令、存档和能力查询。
- [Glk 0.7.6](https://eblong.com/zarf/glk/Glk-Spec-076.html)：窗口、流、输入和可选媒体扩展。
- [Blorb 2.0.5](https://eblong.com/zarf/blorb/)：执行文件与资源容器。
- [Glulxe](https://github.com/erkyrath/glulxe)：差分和存档互操作参考。来源调研保留在 [原调研](glulx-spec-research.ZH.md)。

## VM 核心

证据：[VM](../src/vm.rs)、[Memory](../src/memory.rs)、[存档](../src/vm/save.rs)、[回归矩阵](../src/vm/conformance.rs)。

- [x] 文件头、版本、checksum、ROM/RAM/扩展内存校验；`verify` 检查原始执行映像，不再固定成功。
- [x] 大端访问、ROM 保护、扩缩内存、重叠复制；setmemsize/malloc 超限或预留失败按规范返回失败；栈超限返回错误；零长度 mzero/mcopy 不访问地址，函数帧遵守栈容量。
- [x] opcode 编码和标准寻址、窄局部变量、整数/位/数组/搜索/栈操作及边界；官方 150 条 opcode 分发和操作数数量逐项匹配。
- [x] C0/C1、call/callf/tailcall、return、catch/throw、调用续体。
- [x] Null/filter/Glk I/O、byte/Unicode/Huffman 字符串、间接节点与带参调用；输出续体迭代执行，40,000 次 Huffman 子字符串输出与参考一致。
- [x] 单精度和双精度完整指令族；转换、NaN/Infinity、正负零、容差和双 word 存储次序；不跳转时也消费分支操作数。
- [x] `save/restore`：IFZS 身份、CMem/UMem、Stks、MAll、失败不替换状态和保护区；与 Glulxe 双向互读。
- [x] `accelfunc/accelparam` 与 Inform 加速函数 1–13；未知函数取消注册，未知参数合法忽略。属性、类、隐私和旧/新对象布局与 Glulxe 差分一致。
- [x] Glulx gestalt 与实现对齐：3.1.3、Float、Double、ExtUndo、加速设置；加速函数 1–13 均声明支持。
- [x] 启动和 `setrandom(0)` 使用系统熵；非零 seed 可复现，正/负/零范围有回归。
- [x] 多级 undo、hasundo/discardundo 正确结果；最多 16 个状态且共用 256 MiB 数据预算，按保留页、栈字节和堆记录数据计费。
- [x] restore/undo/restart 不回滚 RNG、I/O system、字符串表、Glk 对象及保护区定义；restart 保留 undo。
- [x] 堆分配、碎片合并、释放回缩、存档/undo 所有权；1 GiB 内存上限、失败分配及保护区跨扩展内存回归。

## Glk

证据：[窗口树](../src/vm/windows.rs)、[流](../src/vm/streams.rs)、[事件](../src/vm/events.rs)、[呈现](../src/vm/presentation.rs)、[声音](../src/vm/sound.rs)、[GUI](../src/app.rs)。

- [x] pair 窗口树、父子/sibling、排列查询/修改、嵌套布局、关闭子树和 resize/arrange 事件；修改排列不翻转物理子窗顺序，blank/pair 返回零尺寸，文本窗尺寸使用实际字体度量。
- [x] fileref 创建/选择/销毁、文件/内存/资源流，byte/Unicode 读写 char/line/buffer、seek、计数和 echo stream；按编码字节定位/覆盖 UTF-8，关闭流解除 echo 绑定，文件选择检查读取路径并提示已有文件修改。
- [x] 按窗口保存行/字符请求、初始行内容、取消结果、多请求、队列、select/poll、定时器、鼠标和超链接；select_poll 不取走玩家输入，取消输入保留编辑内容，网格原位输入及特殊按键在 GUI 验证。
- [x] Latin1 输入/大小写、Unicode 扩展大小写/titlecase/NFC/NFD；官方 Unicode 和资源流样本与参考输出一致。
- [x] 样式状态、查询与 GUI 文本段呈现；支持段落缩进/悬挂缩进/四种对齐及 text-buffer hints 0–9；网格支持字重、斜体、颜色/反色，保留固定格尺寸。样式查询报告实际值，样式命令沿 echo 链传播。
- [x] text-grid、graphics window、实际窗口布局、图像缩放/裁剪、坐标及窗口类型能力参数；text-buffer 支持三种行内对齐、两侧/重复边栏绕排、flow-break、图片超链接及动态缩放。零尺寸图片不占空间。graphics 画布随窗口裁剪/扩展并填充当前背景，矩形宽高按无符号值裁剪。
- [x] 声道、播放/重复/停止/暂停、音量及渐变、完成通知和多声道播放；MOD/XM/S3M/IT 使用纯 Rust 按需解码，play_multi 在同一立体声采样帧起播。无音频设备时不声明声音能力。
- [x] 日期/时间、资源流、行终止键和回显控制；UTC/local、负时间及日期规范化回归；完整 i32 年字段范围使用 Gregorian 运算，DST 间隙、跳日及远古/未来偏移有隔离时区测试。
- [x] dispatch 的引用/数组/结构体/栈结果、对象生命周期和输出参数；124 个官方 selector 均有分发；未知 selector 记录并返回 0。
- [x] gestalt 逐参数核对；GUI 按真实字体覆盖返回 CharOutput，支持系统/自选字体；headless 不声明图形、鼠标、声音等 GUI 能力。
- [x] 升级 Glk 0.7.6，提供 `image_draw_scaled_ext`；graphics 绘图固定调用时尺寸；text-buffer 根据当前窗口宽度动态重排，支持比例/aspect/maxwidth 规则及透明图像。

## Blorb 与播放器

- [x] FORM/IFRS chunk 边界及 RIdx 校验、索引执行文件、图片/声音/数据资源、FORM 音频资源头。
- [x] iFiction 元数据、Fspc 封面、RDes 图像/声音文字描述及故事信息面板。
- [x] GUI 每 30 秒及正常退出保存会话；不指定故事启动时恢复 VM、Glk、输入等待、图形画布、计时器及声音进度。桌面快照独立于可移植 IFZS。

## 验收与维护

- [x] 270 个库测试和 6 个 CLI 测试通过；另有 6 个性能测试保留为手动测量。按领域的测试矩阵和边界覆盖见验收记录。
- [x] 当前仓库通过 `cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings` 和 `cargo build --release`。
- [x] Glulxercise 综合、单精度、双精度最终整合共 92 个通过段落、三轮全部通过；此前一次随机分布阈值失败亦保留在验收记录。
- [x] Inform 加速函数 1–13 共 94 项结果与 Glulxe 精确一致；合成媒体故事验证图像重排、点击、MOD 完成事件和会话恢复。
- [x] 固定 Glulxe/CheapGlk revision，合成故事及 Adventure 双向存档验证；Unicode/资源流精确规范化输出比较。
- [x] 扩展公开故事回归到 Adventure、Unicode、资源流、输入扩展、日期时间、多窗口和 Sensory Jam；明确区分专项测试、启动冒烟与 GUI 操作。

## 后续审计补充

- [x] 同一文件多个流的读写一致性，避免旧句柄关闭时覆盖新数据。
- [x] 超出 chrono 范围、但 Glk 日期字段仍可表示的时间戳转换。
- [x] IFZS 重复 ANNO/未知 chunk 及重复已知 chunk 的规范处理。
- [x] 精确有限音频重复；Blorb MOD 资源内的 XM/S3M/IT 格式；AIFF/OGG/MP3 逐包解码、编码填充裁剪及连续重采样。
- [x] 损坏图片绘制返回失败，超大目标尺寸的有界裁剪采样，以及 GPU 纹理边长适配。
- [x] Blorb RIdx 首块规则及 RDes 文本描述。

## 本轮完成的播放器能力

- [x] 独立 Blorb 资源挂载：为原始 `.ulx` 指定不含执行文件的资源包，提供公开 API、CLI 参数和 GUI 入口；图片、声音、Data 及资源描述使用选中的资源包。存在 `IFhd` 时按 Glulx 的前 128 字节校验；不存在时允许加载，错误身份/损坏包不能替换现有资源。
- [x] Blorb 身份及参数冲突诊断：内嵌执行文件的 IFhd 与故事一致；显式独立故事加含 Exec 的资源包报告冲突；不混用 Z-machine 身份布局。
- [x] 同名资源自动发现（播放器策略）：在故事目录查找同名 Blorb，明确候选优先级和显式指定优先规则；缺失、损坏及不匹配的候选给出可理解的结果。
- [x] 独立资源的会话恢复：保留资源来源和内容，恢复后图片、声音、Data 仍可用；覆盖故事本身包含资源和外置资源两种情况。
- [x] `SONG` 旧音频格式：解析 `SND<number>` 外部 AIFF 样本引用及 sustain loop，接入现有播放、重复、暂停和恢复路径。此项是 Blorb 明确可选、已废弃的扩展。
- [x] 真实终端宿主：显示成功创建的 text-grid/status 窗口及多窗口布局，支持预填编辑、编辑中定时取消取回当前内容、无需 Enter 且无回显的单字符输入；保留管道自动化驱动模式，并准确区分其能力。
- [x] 实际 light 字重（可选显示增强）：存在可用字体时选择并渲染细字重，让 style_measure/style_distinguish 与实际呈现一致；缺失字体时继续如实报告回退值。

- [x] 散装资源目录（可选播放器便利）：显式选择目录，按记录的 PIC/SND/DATA 命名和格式规则加载资源，覆盖编号解析、类型、路径边界和会话恢复；Glk 允许不提供此入口。

## 待完成验收

- [ ] Windows 实机验收：窗口/字体/DPI、输入和特殊键、文件提示及路径、音频、正常退出和会话恢复；记录系统版本与构建版本。
- [ ] macOS 实机验收：窗口/字体/Retina、输入和特殊键、文件提示及路径、音频、正常退出和会话恢复；记录系统版本与构建版本。
- [ ] 长篇游戏完整流程：固定游戏版本并保存可复现路线，覆盖跨章节状态、游戏存档/读档、undo/restart，以及中途关闭播放器后继续。
- [ ] 扩展媒体样本矩阵：增加 PNG/JPEG 编码变体、采样音频位深/采样率/声道及 tracker 历史变体，逐项记录支持结果和失败行为。
- [ ] 实体音频输出验收：检查同步起播、暂停/恢复、渐变和结束通知与实际输出的对应关系，补充软件采样帧验证之外的证据。

独立资源、终端输入和列出的可选播放器能力已实现并完成本地验收，跨平台验收仍待补充。上述待办是验收工作，不表示对应功能已知缺失。

## 平台验收矩阵

| 场景 | Linux x86_64 | Windows | macOS |
| --- | --- | --- | --- |
| VM/CLI/单元测试 | 已完成；`cargo test --all-targets` | 待实机 | 待实机 |
| GUI 窗口、输入、图形、资源和会话 | 已有本地 GUI 工具验收 | 待实机 | 待实机 |
| 字体、DPI、原生文件对话框 | Linux 结果已记录 | 待实机 | 待实机；需覆盖 Retina |
| TTY 与管道宿主 | 已完成 | 待实机 | 待实机 |
| 实体音频设备 | 未完成；已有软件采样帧证据 | 待实机 | 待实机 |
| 长篇游戏完整流程 | 未完成 | 未完成 | 未完成 |

实机记录应保留操作系统版本、架构、构建 commit、Rust 工具链、显示缩放、字体来源和音频设备；失败也应记录为可复现结果，而不是只保留通过项。

## 现有限制与规范范围

网格按规范保持统一格尺寸，忽略改变格布局的 hints 0–3/6；存在对应 light 字体时使用真实细字重，缺失时回退常规并如实报告。缺少字形返回 CannotPrint，可加载备用字体。MOD/XM/S3M/IT 均支持，未宣称全部历史方言或与特定硬件位精确重放。Blorb 的 Rect/Reso/APal/Loop 属于 Z-machine 范围。VM 内存上限默认为 1 GiB，undo 最多 16 个状态且共用 256 MiB 数据预算，按保留页、栈字节和堆记录数据计费。图片 RGBA 输出受可配置上限约束，默认 256 MiB；解码器分配预算为该上限的两倍。绘制目标尺寸不受此源图像限制。上述资源限额是当前实现策略；undo 不计共享故事映像、当前 VM 地址空间、堆索引及分配器/容器开销。容器读取仍容忍非零 padding 和未索引 GLUL 回退，详见复核文档，不宣称拒绝全部非法容器。
