# Glulx 规范兼容性 checklist

核对时间：2026-09-08 04:45 UTC。基线为 `31bb75c`（feat: implement Glulx heap allocation and correct Glk stack results）。交付前已再次读取关联任务“实现带图形界面的 Glulx VM”的状态，确认本轮完成，并对新提交重新检查源码与测试。此清单仍是一份时间点快照。

`[x]` 表示已有对应实现，不表示通过完整规范认证；`[ ] 缺失` 表示未找到实现；`[ ] 部分` 表示只有部分路径；`[ ] 待验证` 表示已有代码但仍需规范边界测试。不要根据任务计划或仅有 opcode 分支就将完整能力勾选。规范来源与访问情况见 [规范调研](glulx-spec-research.md)。

## 规范边界和公开来源

- [Glulx 官方入口](https://eblong.com/zarf/glulx/) / [规范正文](https://eblong.com/zarf/glulx/Glulx-Spec.html)：VM、指令、内存、调用栈、字符串、存档和能力查询。本清单以项目目标 Glulx 3.1.3 为基线。
- [Glk 官方规范](https://eblong.com/zarf/glk/Glk-Spec-076.html)：窗口、流、输入、图像、声音等宿主 I/O；应与 VM 核心分开评估。
- [Blorb 官方入口](https://eblong.com/zarf/blorb/)：可执行文件与媒体资源容器；支持 `GLUL` 不等于支持全部媒体。
- [Glulxe 参考实现](https://github.com/erkyrath/glulxe)：用于差分测试，不能替代规范正文。

双精度、具体加速函数及部分 Glk 模块具有能力探测/可选性（加速设置指令本身是 3.1.1+ 的要求）：未实现不应一概解释为所有游戏都无法运行。自动存档、封面展示、翻译属于播放器需求，不作为 Glulx 核心合规条件。

## VM 核心：已有实现

证据入口：[Story](../src/story.rs)、[Memory](../src/memory.rs)、[Vm](../src/vm.rs) 的 `step`、`operand_count`、各辅助方法及文件尾部测试。

- [x] 文件头、版本范围、checksum、ROM/RAM/扩展内存加载与基本布局校验（`StoryHeader::validate`）。
- [x] 大端 byte/short/word 访问、ROM 写保护、扩缩内存、`mzero/mcopy` 和重叠复制（`Memory`）。
- [x] 可变长 opcode、标准加载/存储寻址模式（`fetch_opcode/fetch_operand`）。
- [x] 整数运算、位操作、条件分支、数组访问和栈操作（`step`）。
- [x] C0/C1 函数、call/callf、tailcall、return、catch/throw 与调用续体。
- [x] linearsearch、binarysearch、linkedsearch；已有基本搜索测试。
- [x] Null、filter、Glk 三种 I/O system；filter 字符串/数字恢复已有测试。
- [x] byte/Unicode/Huffman 字符串及嵌入调用、字符串表访问。
- [x] 单精度指令分支；包含 `fmod` 和浮点比较测试。
- [x] 单级 `saveundo/restoreundo`，`hasundo/discardundo`；已有 undo 行为测试。
- [x] `restart/protect`，内存保护区恢复；已有保护区测试。
- [x] `malloc/mfree`、堆开启时禁止 `setmemsize`、最后释放后回缩、undo/restart 堆状态处理；已提交于 `31bb75c`，含碎片复用、扩缩限制及 undo/restart 堆所有权测试，不能继续列作完全缺失。

## VM 核心：剩余工作

- [ ] **缺失：持久存档 `save/restore`（0x123/0x124）。** `operand_count` 和 `step` 未包含两条指令。实现 Glulx 的 Quetzal/IFZS 存档约定，包括身份检查、RAM、栈/续体、扩展内存与堆状态；验收应包含与参考解释器互读存档、失败返回值及 `protect`。
- [ ] **缺失：加速设置指令 `accelfunc/accelparam`（0x180/0x181）。** 未找到分支；`gestalt` 9/10 返回 0。规范要求 3.1.1+ 支持设置指令，未知函数/参数可以不操作；具体 Inform 加速函数才是可选优化。当前声明 3.1.3 与缺少设置指令不一致。
- [ ] **缺失：双精度浮点指令族（0x200–0x239 中规范定义的指令）。** `operand_count/step` 未见对应实现。覆盖转换、算术、取整、余数、超越函数、比较和 NaN/Infinity 分支，并核对双 word 的高低位与多目标写入顺序。
- [ ] **部分：能力声明。** `gestalt` 报告 3.1.3，Float 为 1，但 ExtUndo（12）和 Double（13）仍落入默认 0；ExtUndo 指令已经存在，需统一实现与声明。Glulx 版本号不能当作可选能力全部可用的证明。
- [ ] **规范偏差：random/setrandom。** 当前固定初始 seed，`setrandom` 使用 `seed.max(1)`；规范要求启动和 seed=0 使用尽可能不可预测的模式。补齐熵来源，并测试正/负/零范围与非零 seed 的确定序列。
- [ ] **待验证：verify。** `verify` 当前固定返回 0；加载时确有 checksum 检查。需按规范确认运行时检查对象与失败语义，不能把常量成功视为完成验证。
- [ ] **待验证：数值边界。** NaN/Infinity、正负零、溢出转换、整数极值除法、移位边界、浮点容差。现有少量测试不能覆盖整个指令族。
- [ ] **规范偏差：undo 的非存档状态。** `UndoState` 和 `restoreundo` 当前保存/恢复 RNG、I/O system/rock、字符串表；规范“State Not Saved”明确这些状态不随 restore/restoreundo/restart 回退。修正状态边界并增加回归；Glk 对象和保护区定义也应保持独立。
- [ ] **待验证：堆与恢复。** 最新提交已测试碎片合并复用、扩缩限制、非法释放、零/溢出分配失败及 undo/restart 堆所有权与样本数据；继续补充资源耗尽、保护区跨扩展内存等边界。
- [ ] **待验证：解码/调用/字符串完整边界。** 覆盖所有寻址模式、局部变量宽度与对齐、栈边界、特殊返回分支，以及全部 Huffman 节点和间接/带参调用。

## Glk：已有基础与剩余工作

证据入口：[Vm](../src/vm.rs) 的 `glk/glk_gestalt`、[GUI](../src/app.rs) 的 `poll_graphics` 和显示逻辑。下列未勾选条目不代表每个扩展都是 Glk 必选项；启用能力必须与 gestalt 一致。

- [x] 基础窗口/流句柄、文本输出、行/字符输入、select，以及 byte/Unicode memory stream 输出路径。
- [x] text-grid 独立文本缓冲；图像读取、缩放、graphics window 填充/清除/关闭及 GUI 绘制路径。
- [ ] **部分：完整窗口树。** 已有打开/关闭/尺寸逻辑；排列查询/更新、父子/sibling、嵌套 pair、resize/arrange 事件仍需补齐和验证。
- [ ] **部分：流。** 文件流、fileref 创建/选择/销毁、读取 char/line/buffer、resource stream 及 Unicode 对应调用未完整实现；memory stream 已有部分能力，不能笼统写“流未实现”。
- [ ] **部分：输入事件。** 目前主要是单个 line/char request；补齐取消结果、初始行内容、多窗口请求、事件排队/轮询、鼠标、定时器、超链接和相关扩展能力。
- [ ] **部分：字符与 Unicode。** Unicode 输出及部分输入已有；大小写转换目前只处理 ASCII，Unicode 大小写/规范化调用有返回 0 占位；核对 byte 输入编码与 Unicode API 完整覆盖。
- [ ] **部分：样式。** `set_style`、style hints 等选择器存在无操作分支；补齐所选支持范围、样式查询及 GUI 呈现。
- [ ] **部分：图形/text-grid 合规。** 已能绘制，不再属于完全缺失；仍需布局、窗口尺寸、裁剪/坐标、文本流内图像和 capability 参数一致性测试。
- [ ] **缺失：声音。** 声道、播放/停止、音量、通知事件和对应资源支持。
- [ ] **缺失或待逐 selector 核对：其他扩展。** 日期/时间、资源流、行终止键/回显控制等，按所选 Glk 版本及 gestalt 逐项确定实现或明确拒绝。
- [ ] **部分：Glk dispatch 合同。** 当前手写 selector 分支；最新提交已补 Glk 栈参数/stream_close 栈结果测试，未知调用仍统一记录并返回 0。补齐数组/结构体/栈参数、对象生命周期、输出参数和每个已声明能力的准确语义；探测调用返回 0 不能代替所有调用的实现。
- [ ] **待验证：Glk gestalt。** 当前报告 0.7.5，部分能力使用范围匹配且忽略 argument；检查字符可打印性、窗口类型相关图形能力和实际实现是否一致。

- [ ] **版本升级项：Glk 0.7.6。** 当前 VM 声明 0.7.5；若升级，增加 `glk_image_draw_scaled_ext`、对应 gestalt 和文本窗口图片宽度适配规则。这是独立升级项，不当作 0.7.5 必选缺口。

## Blorb 与播放器（独立于 VM 核心）

- [x] `FORM/IFRS` 的 `GLUL` 提取和 `RIdx` 图片资源索引；已有加载测试。
- [ ] **部分：Blorb 资源覆盖。** 除已实现图片路径外，继续核对声音、资源类型/尺寸元数据和异常容器处理。
- [ ] **产品缺失：** 自动存档/会话恢复、封面与 iFiction 元数据展示。GUI 打开/重启/停止、滚屏、设置和翻译已经存在。

## 验收与维护

- [x] 本次运行 `RUSTC_WRAPPER= cargo test --all-targets`：30 tests passed；这是已有回归集通过，不是完整 spec conformance 测试。
- [ ] 建立按规范条目组织的 opcode/寻址/边界测试矩阵；每个完成项记录覆盖范围。
- [ ] 对 Glulxe/Git 跑相同 story 和输入，比较输出、退出行为、存档互操作；保留工具版本和样本来源。
- [ ] 扩展真实游戏回归，覆盖存取档、undo、Unicode、窗口/图形与声音；启动成功不能证明游戏可完整通关。

更新规则：以落地源码和测试为准，标注核对时间/commit；关联任务正在实现的条目，只有再次检查源码后才变更状态。保留“实现存在”和“完整验证”两个层次。优先补持久存档及其 Glk 文件接口，再处理能力声明与 VM 边界；双精度、加速及媒体按目标游戏需求推进。
