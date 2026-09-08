# Glulx 规范兼容性 checklist

更新：2026-09-08。本轮在 `31bb75c` 基线上实现并验收原清单中的缺口；以下描述本轮实现快照。`[x]` 表示实现及列出的验证已完成，不表示穷尽规范认证。详细测试、样本版本、复现命令见 [验收记录](glulx-validation.md)，实现限制见 [兼容性](compatibility.md)。

## 规范边界

- [Glulx 3.1.3](https://eblong.com/zarf/glulx/Glulx-Spec.html)：VM、指令、存档和能力查询。
- [Glk 0.7.6](https://eblong.com/zarf/glk/Glk-Spec-076.html)：窗口、流、输入和可选媒体扩展。
- [Blorb](https://eblong.com/zarf/blorb/)：执行文件与资源容器。
- [Glulxe](https://github.com/erkyrath/glulxe)：差分和存档互操作参考。来源调研保留在 [原调研](glulx-spec-research.md)。

## VM 核心

证据：[VM](../src/vm.rs)、[Memory](../src/memory.rs)、[存档](../src/vm/save.rs)、[回归矩阵](../src/vm/conformance.rs)。

- [x] 文件头、版本、checksum、ROM/RAM/扩展内存校验；`verify` 检查原始执行映像，不再固定成功。
- [x] 大端访问、ROM 保护、扩缩内存、重叠复制；资源耗尽有明确失败结果。
- [x] opcode 编码和标准寻址、窄局部变量、整数/位/数组/搜索/栈操作及边界。
- [x] C0/C1、call/callf/tailcall、return、catch/throw、调用续体。
- [x] Null/filter/Glk I/O、byte/Unicode/Huffman 字符串、间接节点与带参调用。
- [x] 单精度和双精度完整指令族；转换、NaN/Infinity、正负零、容差和双 word 存储次序；不跳转时也消费分支操作数。
- [x] `save/restore`：IFZS 身份、CMem/UMem、Stks、MAll、失败不替换状态和保护区；与 Glulxe 双向互读。
- [x] `accelfunc/accelparam` 正确消费参数；不支持的优化函数合法忽略。
- [x] Glulx gestalt 与实现对齐：3.1.3、Float、Double、ExtUndo、加速设置；具体加速函数返回不支持。
- [x] 启动和 `setrandom(0)` 使用系统熵；非零 seed 可复现，正/负/零范围有回归。
- [x] 多级 undo、hasundo/discardundo 正确结果；最多 16 个状态且共用 64 MiB 预算。
- [x] restore/undo/restart 不回滚 RNG、I/O system、字符串表、Glk 对象及保护区定义；restart 保留 undo。
- [x] 堆分配、碎片合并、释放回缩、存档/undo 所有权；256 MiB 内存上限、失败分配及保护区跨扩展内存回归。

## Glk

证据：[窗口树](../src/vm/windows.rs)、[流](../src/vm/streams.rs)、[事件](../src/vm/events.rs)、[呈现](../src/vm/presentation.rs)、[声音](../src/vm/sound.rs)、[GUI](../src/app.rs)。

- [x] pair 窗口树、父子/sibling、排列查询/修改、嵌套布局、关闭子树和 resize/arrange 事件。
- [x] fileref 创建/选择/销毁、文件/内存/资源流，byte/Unicode 读写 char/line/buffer、seek、计数和 echo stream。
- [x] 按窗口保存行/字符请求、初始行内容、取消结果、多请求、队列、select/poll、定时器、鼠标和超链接。
- [x] Latin1 输入/大小写、Unicode 扩展大小写/titlecase/NFC/NFD；官方 Unicode 和资源流样本与参考输出一致。
- [x] 样式状态、查询与 GUI 文本段呈现；明确支持 text-buffer hints 3–9，忽略可选段落 hints 0–2 和 grid hints。
- [x] text-grid、graphics window、实际窗口布局、图像缩放/裁剪、坐标及窗口类型能力参数；文本流内图像明确返回不支持。
- [x] 声道、播放/重复/停止/暂停、音量及渐变、完成通知和多声道播放；无音频设备时不声明声音能力。
- [x] 日期/时间、资源流、行终止键和回显控制；UTC/local、负时间及日期规范化回归。
- [x] dispatch 的引用/数组/结构体/栈结果、对象生命周期和输出参数；未知 selector 记录并返回 0。
- [x] gestalt 逐参数核对；headless 不声明图形、鼠标、声音等 GUI 能力。
- [x] 升级 Glk 0.7.6，提供 `image_draw_scaled_ext`；仅声明 graphics window 绘图，文本窗口图像宽度规则不适用。

## Blorb 与播放器

- [x] 严格 FORM/IFRS chunk 边界及 RIdx 校验、索引执行文件、图片/声音/数据资源、FORM 音频资源头。
- [x] iFiction 元数据、Fspc 封面及故事信息面板。
- [x] GUI 每 30 秒及正常退出保存会话；不指定故事启动时恢复 VM、Glk、输入等待、图形画布、计时器及声音进度。桌面快照独立于可移植 IFZS。

## 验收与维护

- [x] 56 个 Rust 测试通过；按领域的测试矩阵和边界覆盖见验收记录。
- [x] Glulxercise 综合、单精度、双精度共 92 个通过段落、三轮全部通过。
- [x] 固定 Glulxe/CheapGlk revision，合成故事及 Adventure 双向存档验证；Unicode/资源流精确规范化输出比较。
- [x] 扩展公开故事回归到 Adventure、Unicode、资源流、输入扩展、日期时间、多窗口和 Sensory Jam；明确区分专项测试、启动冒烟与 GUI 操作。

## 明确保留的可选范围与验证限制

具体 Inform 加速函数、MOD tracker 音乐、text-buffer 内嵌图像未实现且不声明支持。样式 hints 可以被宿主忽略；当前字体回退及 grid 排版受 egui 限制。Sound2 多通道启动未验证采样级同步。Linux GUI 验收不能替代 Windows/macOS 实机验证，公开游戏冒烟不等于完整通关。上述限制不是通过勾选被消除的能力；未来扩展需增加实现及相应验收。
