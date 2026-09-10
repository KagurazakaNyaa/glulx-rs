# Glulx 公开规范来源与核对笔记

[English](glulx-spec-research.md) | [中文](glulx-spec-research.ZH.md)

复核日期：2026-09-10。已实际读取官方 Glulx/Glk/Blorb 首页及 Glulx 3.1.3、Glk 0.7.6、Blorb 2.0.5 正文。具体实现状态见[兼容性说明](compatibility.ZH.md)和[当前规范复核](glulx-remaining-spec-audit.ZH.md)；这里记录规范依据，避免将实现计划或其他文档误当作规范。

## 正确入口与版本

| 范围 | 官方来源 | 本次核实结果 |
| --- | --- | --- |
| Glulx VM | [首页](https://eblong.com/zarf/glulx/)、[HTML](https://eblong.com/zarf/glulx/Glulx-Spec.html)、[Markdown](https://eblong.com/zarf/glulx/Glulx-Spec.md) | 当前版本 3.1.3 |
| Glk I/O | [首页](https://eblong.com/zarf/glk/)、[HTML](https://eblong.com/zarf/glk/Glk-Spec-076.html)、[Markdown](https://eblong.com/zarf/glk/Glk-Spec-076.md) | 当前版本 0.7.6；本仓库已声明并实现 0.7.6 |
| Blorb 容器 | [首页](https://eblong.com/zarf/blorb/)、[规范](https://eblong.com/zarf/blorb/Blorb-Spec.html) | 版本 2.0.5；后续已核对正文的资源包、IFhd、媒体格式及可选范围 |

注意：[旧小写 URL](https://eblong.com/zarf/glulx/glulx-spec.html) 仍可访问，但本次返回的是 **3.1.2** 分页规范。新增文档应使用上表的大小写正确入口，避免漏掉双精度和扩展 undo。

## 可以直接用于 checklist 的规范要求

| 规范部分 | 对实现/验收的意义 |
| --- | --- |
| [The Machine](https://eblong.com/zarf/glulx/Glulx-Spec.html) | 核对内存图、文件头、栈帧、call stub、指令编码与对象类型；不能只统计 opcode 数量。 |
| [Save-Game Format](https://eblong.com/zarf/glulx/Glulx-Spec.html#saveformat) | 存档为 Quetzal 变体：CMem/UMem 含当前内存尺寸，保存 RAMSTART 到当前内存末尾；栈包含续体，MAll 保存堆，IFhd 使用前 128 字节。 |
| [State Not Saved，位于存档章节](https://eblong.com/zarf/glulx/Glulx-Spec.html#saveformat) | Glk 状态、保护区定义、RNG 内部状态、I/O system 和字符串表地址不随 restart/restore/restoreundo 回退。当前实现及回归已按此区分状态边界。 |
| [Random Number Generator](https://eblong.com/zarf/glulx/Glulx-Spec.html#opcodes_rand) | 非零 seed 必须产生可重复序列；seed=0 和初始启动为非确定模式；RNG 不属于存档状态。 |
| [Accelerated Functions](https://eblong.com/zarf/glulx/Glulx-Spec.html#opcodes_accel) | 可以不提供任何具体加速函数；未知加速函数编号解除该地址的加速注册；未知参数设置忽略。必须区分设置指令与实际优化函数。 |
| [Gestalt / Miscellaneous](https://eblong.com/zarf/glulx/Glulx-Spec.html#opcodes_misc) | Acceleration(9) 对 3.1.1+ 必须为真；AccelFunc(10) 查询具体函数；Float(11)、ExtUndo(12)、Double(13) 分别查询对应能力。Glulx Unicode 不能代替 Glk Unicode 探测。 |
| [Double-Precision Math](https://eblong.com/zarf/glulx/Glulx-Spec.html#opcodes_double)、[Comparisons](https://eblong.com/zarf/glulx/Glulx-Spec.html#opcodes_doublebranch) | 3.1.3 新增双精度；解释器可不支持，但必须准确报告 Double 能力。覆盖指令定义在 0x200–0x239 范围，并非所有中间数值都有指令。 |
| [Game State，见指令正文](https://eblong.com/zarf/glulx/Glulx-Spec.html) | hasundo/discardundo 为 3.1.3 扩展，以 ExtUndo 查询。verify 检查文件长度/checksum；规范指出解释器可以在启动时先自动验证。 |
| [Glk 0.7.6 正文](https://eblong.com/zarf/glk/Glk-Spec-076.html) | Glk 与 VM 是独立兼容性层，包含窗口、流、事件、字符、图像、声音及可选模块。0.7.6 新增 image_draw_scaled_ext，并改变文本窗口中过宽图片的适配行为；仓库已完成这项升级，其测试证据见验收记录。 |

## 实现核对方法

将每条要求映射到 [vm.rs](../src/vm.rs) 的 `operand_count/step/gestalt/glk/glk_gestalt`、[memory.rs](../src/memory.rs)、[story.rs](../src/story.rs) 与 [app.rs](../src/app.rs)。已有分支、返回固定值、无操作占位和完整可用能力应分开记录。测试通过只证明已执行用例，不证明所有规范边界。

行为差分使用官方入口链接的 [Glulxe 参考实现](https://github.com/erkyrath/glulxe) 以及项目既有参考 [Git](https://github.com/DavidKinder/Git)。当前版本和验收范围见[验收记录](glulx-validation.ZH.md)。
