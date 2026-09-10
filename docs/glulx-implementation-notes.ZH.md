# Glulx 存档与数值实现笔记

[English](glulx-implementation-notes.md) | [中文](glulx-implementation-notes.ZH.md)

核对日期：2026-09-10。依据 [Glulx 3.1.3 正文](https://eblong.com/zarf/glulx/Glulx-Spec.html)、[Glulxe serial.c](https://github.com/erkyrath/glulxe/blob/master/serial.c) 和 [exec.c](https://github.com/erkyrath/glulxe/blob/master/exec.c)。当前参考 revision 为 `56ab8743bab565de307bd892c555d8d8897ed517`；存档互操作覆盖见[当前验收记录](glulx-validation.ZH.md)。

## IFZS 持久存档

根据 [Save-Game Format](https://eblong.com/zarf/glulx/Glulx-Spec.html#saveformat) 和 `serial.c:perform_save/perform_restore`：所有整数为大端，文件外层是 `FORM <u32 文件长度减8> IFZS`。每个 chunk 是四字节 ID、u32 内容长度、内容；奇数长度的内容后补一个零，补齐字节不计入该 chunk 长度，但计入 FORM 长度。未知 chunk 应跳过。

| Chunk | 内容与恢复要求 |
| --- | --- |
| `IFhd` | 故事内存最初 128 字节。与运行中故事的 ROM 对比，拒绝其他故事的存档。 |
| `UMem` | u32 当前内存尺寸，随后是 `RAMSTART..当前内存尺寸` 的原始字节；不保存 ROM。 |
| `CMem` | u32 当前内存尺寸，随后是 RAM 与初始故事的 XOR 差值，经零串压缩。`EXTSTART` 之后的原始故事字节按零处理。 |
| `Stks` | 完整标准栈字节，末尾包含 save 压入的四 word 续体。 |
| `MAll` | u32 heap start、u32 已分配块数，随后每块的 u32 地址、u32 长度；顺序不限。无堆时可省略或保存 `0,0`。参考 Glulxe 还会写空的 MAll chunk，读取时应兼容。 |

**写出 CMem 以兼容 Glulxe。** 当日参考实现的 `perform_restore` 只识别 CMem，没有 UMem 分支，虽然规范允许 UMem。CMem 编码见 `serial.c:write_memstate/read_memstate`：非零差值直接写一个字节；连续 1–256 个零编码为 `00 (数量-1)`，更长的零串分段；末尾零串可以完全省略，读取结束后余下差值都是零。解码应检查截断标记、尺寸及越界，而不是先修改运行中的内存再发现错误。

根据 [Game State 指令](https://eblong.com/zarf/glulx/Glulx-Spec.html) 和 `exec.c:op_save/op_restore`，save 的流是程序已打开的可写 Glk stream；提示路径和创建 fileref 属于 Glk API/游戏责任。Null/filter I/O 下 save 非法。保存前压入 `(DestType, DestAddr, 下一条指令PC, 当前FramePtr)`，成功后弹出并写 0，失败写 1；恢复成功则弹出存档里的续体并向 **save 的目的地** 写 `0xffffffff`。restore 指令自己的目的地只在恢复失败时写 1。

## 栈与恢复状态边界

根据 [The Stack](https://eblong.com/zarf/glulx/Glulx-Spec.html#stack) 与 `serial.c:write_stackstate/read_stackstate`：

- 栈以字节寻址，从 0 向上生长；标准帧依次为 u32 FrameLen、u32 LocalsPos、局部格式 byte pairs、四字节对齐 padding、按自然宽度对齐的 locals、padding、u32 值栈及 call stubs。
- 局部格式为 `(width,count)`，width 为 1/2/4，以 `(0,0)` 终止，格式段补齐至四字节。padding 写零；locals 依其真实宽度写大端。仓库 `Stack.bytes` 已使用大端，可直接输出标准栈，但仍应保证 padding 合法。
- 最后一个 word 是当前 frame pointer。恢复可从 stack end 开始，读取 `end-4` 得到 frame start，再沿 frame start 回溯至 0，验证帧长、locals 格式/范围、值栈对齐、总栈限制。完成解析后才替换 VM 状态。
- 常规 stub 的 DestType 为 0（丢弃）、1（内存）、2（局部）、3（压栈）。输出续体另有 10/11/12/13/14，必须保留，不能只序列化当前 PC 和 locals。

根据规范的 [State Not Saved](https://eblong.com/zarf/glulx/Glulx-Spec.html#saveformat)，restart/restore/restoreundo 都**不回滚** RNG 内部状态、I/O system（及其 rock）、当前字符串表、Glk 对象及当前输出流、保护区定义。保护区内存正常写入存档；恢复时保留恢复前当前保护区覆盖的内存内容。保存结果目的地即使处于保护区也必须正常写入返回值。restart 重置内存尺寸、堆和执行栈，但仍遵循上述例外。

`hasundo` 的返回值是 **0 表示存在可恢复状态，1 表示不存在**；`discardundo` 在无 undo 时无操作。`verify` 检查原始游戏文件长度和 checksum，而不是游戏修改后的 RAM；参考 `serial.c:perform_verify` 对文件长度、EXTSTART、一致性及求和进行检查。[指令依据](https://eblong.com/zarf/glulx/Glulx-Spec.html)

## 双精度 opcode 映射与边界

根据 [Double-Precision Math](https://eblong.com/zarf/glulx/Glulx-Spec.html#opcodes_double)，输入 pair 的顺序是 **高 word、低 word**；输出顺序是 **低 word、高 word**，依次写出，确保两个 stack store 后可以直接由下一条指令读取。以下 L/S 数量均按 32-bit word 计：

| Opcode | 指令 | 操作数 |
| --- | --- | --- |
| 0x200, 0x203 | numtod, ftod | L S S |
| 0x201, 0x202, 0x204 | dtonumz, dtonumn, dtof | L L S |
| 0x208, 0x209 | dceil, dfloor | L L S S |
| 0x210–0x215 | dadd, dsub, dmul, ddiv, dmodr, dmodq | L L L L S S |
| 0x218–0x21a | dsqrt, dexp, dlog | L L S S |
| 0x21b | dpow | L L L L S S |
| 0x220–0x225 | dsin, dcos, dtan, dasin, dacos, datan | L L S S |
| 0x226 | datan2 | L L L L S S |
| 0x230, 0x231 | jdeq, jdne | L L L L L L L |
| 0x232–0x235 | jdlt, jdle, jdgt, jdge | L L L L L |
| 0x238, 0x239 | jdisnan, jdisinf | L L L |

未列的空洞没有定义；不要按连续范围接受它们。Double 是可选能力，完整提供后才将 gestalt 13 设置为 1。[opcode 表与 gestalt](https://eblong.com/zarf/glulx/Glulx-Spec.html)

双精度沿用 [浮点规则](https://eblong.com/zarf/glulx/Glulx-Spec.html#opcodes_float) 和 [比较规则](https://eblong.com/zarf/glulx/Glulx-Spec.html#opcodes_doublebranch)：

- `dtonumz/dtonumn` 在溢出、Infinity 和 NaN 时按 sign bit 饱和到 `0x7fffffff` / `0x80000000`。Rust 的裸 float-to-int cast 把 NaN 转为 0，因此需显式处理。半整数如何取整由实现决定。
- `dmodr` 使用浮点余数，保留被除数符号；`dmodq` 的参考实现先算 `r=fmod(a,b)`，再算 `(a-r)/b`，结果为零时以两个输入符号 XOR 修正零的符号。不要简单假设直接 `trunc(a/b)` 在所有接近整数的商上与参考一致。除数无限且被除数有限时，余数为被除数、商为适当符号的零；被除数无限或除数零时两者为 NaN。
- 容差等于判断先排除任意 NaN；同号 Infinity 相等，异号 Infinity 即使容差为 Infinity 也不等；其他情况检查 `abs(a-b)<=abs(delta)`。不等分支应是这一整体谓词的反值。
- floor/ceil 保留负零；sqrt(-0) 为 -0；log(±0) 为 -Infinity。`pow(1,NaN)`、`pow(NaN,±0)`、`pow(-1,±Infinity)` 均为 1。
- atan2 的参数为 `(y,x)`，正负零和无限参数的象限遵循规范。主机 math 通常已支持，但应以测试确认。

## 建议验收样本

以下是依据上述格式约束推导的测试设计，不是已完成测试声明：

- RAM 与扩展内存差值、零串 1/256/257、全零尾段、未知奇数 chunk、损坏/截断 chunk、错误 IFhd。
- 嵌套函数、mixed-width locals、栈结果目的地、内存/局部目的地、输出 filter 续体中的 save，堆碎片与无序 MAll。
- 恢复时保护区跨 RAM/扩展内存、恢复失败保持运行状态、恢复后 RNG/I/O/string-table/Glk 状态独立。
- 两个输出都压栈、输出目的地重叠、带符号 NaN 转换、±0、Infinity 容差、整数上下界、mod 商接近整数。
- 使用同一故事分别在本解释器和 Glulxe 保存并由另一方恢复，比较恢复后的输出和返回码；保留参考源码 revision 和实际输入记录。
