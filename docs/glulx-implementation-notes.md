# Glulx Save and Numeric Implementation Notes

[English](glulx-implementation-notes.md) | [中文](glulx-implementation-notes.ZH.md)

Reviewed: 2026-09-10. Based on the [Glulx 3.1.3 specification](https://eblong.com/zarf/glulx/Glulx-Spec.html) and Glulxe's [serial.c](https://github.com/erkyrath/glulxe/blob/master/serial.c) and [exec.c](https://github.com/erkyrath/glulxe/blob/master/exec.c). The current reference revision is `56ab8743bab565de307bd892c555d8d8897ed517`; save interoperability coverage is recorded in the [current validation record](glulx-validation.md).

## IFZS Persistent Saves

According to [Save-Game Format](https://eblong.com/zarf/glulx/Glulx-Spec.html#saveformat) and `serial.c:perform_save/perform_restore`, all integers are big-endian, and the outer file structure is `FORM <u32 file length minus 8> IFZS`. Each chunk contains a four-byte ID, a u32 content length, and its content. Odd-length content is followed by a zero padding byte, excluded from the chunk length but included in the FORM length. Unknown chunks should be skipped.

| Chunk | Content and restoration requirements |
| --- | --- |
| `IFhd` | The first 128 bytes of story memory. Compare against the running story's ROM to reject saves from other stories. |
| `UMem` | A u32 current memory size, followed by raw bytes from `RAMSTART` to the current memory size; ROM is not saved. |
| `CMem` | A u32 current memory size, followed by zero-run-compressed XOR differences between RAM and the initial story. Original story bytes beyond `EXTSTART` are treated as zero. |
| `Stks` | Complete standard stack bytes, ending with the four-word continuation pushed by save. |
| `MAll` | A u32 heap start and u32 allocated-block count, then a u32 address and u32 length for each block, in any order. With no heap, omit it or save `0,0`. The reference Glulxe also writes empty MAll chunks, which readers should accept. |

**Write CMem for Glulxe compatibility.** The reference implementation's `perform_restore` at the time only recognized CMem, with no UMem branch, although the specification permits UMem. For encoding, see `serial.c:write_memstate/read_memstate`: write nonzero differences as single bytes; encode 1–256 consecutive zeros as `00 (count-1)`, splitting longer runs. A trailing zero run may be omitted entirely; all remaining differences after the input ends are zero. Decoding should validate truncated markers, sizes, and bounds before modifying live memory.

According to the [Game State instructions](https://eblong.com/zarf/glulx/Glulx-Spec.html) and `exec.c:op_save/op_restore`, save uses a writable Glk stream already opened by the program; prompting for a path and creating a fileref are Glk API/game responsibilities. Save is illegal under Null/filter I/O. Before saving, push `(DestType, DestAddr, next instruction PC, current FramePtr)`; then pop it and write 0 on success, or write 1 on failure. Successful restore pops the saved continuation and writes `0xffffffff` to **save's destination**. The restore instruction's own destination receives 1 only on restoration failure.

## Stack and Restored State Boundaries

According to [The Stack](https://eblong.com/zarf/glulx/Glulx-Spec.html#stack) and `serial.c:write_stackstate/read_stackstate`:

- The stack is byte-addressed and grows upward from 0. A standard frame contains u32 FrameLen, u32 LocalsPos, local-format byte pairs, padding to four-byte alignment, naturally aligned locals, padding, and a u32 value stack with call stubs.
- Local formats are `(width,count)`, with widths 1/2/4 and a `(0,0)` terminator; the format section is padded to four bytes. Write zero padding and big-endian locals at their actual widths. The repository's `Stack.bytes` is already big-endian and can emit the standard stack directly, provided padding remains valid.
- The last word is the current frame pointer. Restoration can start at the stack end, read `end-4` for the frame start, and walk backward through frame starts to 0, validating frame lengths, local formats/ranges, value-stack alignment, and the total stack limit. Replace VM state only after parsing completes.
- Normal stubs use DestType 0 (discard), 1 (memory), 2 (local), or 3 (push). Output continuations also use 10/11/12/13/14 and must be preserved; serializing only the current PC and locals is insufficient.

Under [State Not Saved](https://eblong.com/zarf/glulx/Glulx-Spec.html#saveformat), restart/restore/restoreundo **do not roll back** the internal RNG state, I/O system and its rock, current string table, Glk objects and current output stream, or protection definition. Protected memory is written to the save normally; restoration retains the current contents covered by the protection range immediately before restore. A save-result destination inside that range must still receive its return value. Restart resets memory size, heap, and execution stack while observing these exceptions.

`hasundo` returns **0 when a restorable state exists, and 1 otherwise**; `discardundo` does nothing when no undo exists. `verify` checks the original game file's length and checksum, not game-modified RAM. See `serial.c:perform_verify` for file length, EXTSTART, consistency, and summation checks. [Instruction reference](https://eblong.com/zarf/glulx/Glulx-Spec.html)

## Double-Precision Opcode Mapping and Edge Cases

According to [Double-Precision Math](https://eblong.com/zarf/glulx/Glulx-Spec.html#opcodes_double), input pairs are ordered **high word, low word**; outputs are written sequentially as **low word, high word**, so the next instruction can directly consume two stack stores. L/S counts below are in 32-bit words:

| Opcode | Instruction | Operands |
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

Unlisted gaps are undefined; do not accept them as part of a continuous range. Double is optional; set gestalt 13 to 1 only when fully implemented. [Opcode table and gestalt](https://eblong.com/zarf/glulx/Glulx-Spec.html)

Double precision follows the [floating-point rules](https://eblong.com/zarf/glulx/Glulx-Spec.html#opcodes_float) and [comparison rules](https://eblong.com/zarf/glulx/Glulx-Spec.html#opcodes_doublebranch):

- On overflow, Infinity, or NaN, `dtonumz/dtonumn` saturate to `0x7fffffff` / `0x80000000` according to the sign bit. Rust's raw float-to-int cast maps NaN to 0, so explicit handling is required. Half-integer rounding is implementation-defined.
- `dmodr` uses floating-point remainder and retains the dividend's sign. The reference `dmodq` computes `r=fmod(a,b)`, then `(a-r)/b`, correcting a zero result's sign with the XOR of the input signs. Do not assume direct `trunc(a/b)` always matches near integral quotients. With an infinite divisor and finite dividend, the remainder is the dividend and the quotient is appropriately signed zero; an infinite dividend or zero divisor makes both NaN.
- Tolerance equality first rejects any NaN. Infinities of the same sign are equal; opposite infinities are unequal even with infinite tolerance. Otherwise, check `abs(a-b)<=abs(delta)`. The inequality branch must negate this entire predicate.
- floor/ceil preserve negative zero; sqrt(-0) is -0; log(±0) is -Infinity. `pow(1,NaN)`, `pow(NaN,±0)`, and `pow(-1,±Infinity)` are all 1.
- atan2 takes `(y,x)`; quadrants for signed zero and infinite arguments follow the specification. Host math usually supports this, but tests should confirm it.

## Suggested Validation Cases

These are test designs derived from the format constraints above, not claims of completed testing:

- RAM and extended-memory differences; zero runs of 1/256/257; all-zero tails; unknown odd-length chunks; corrupt/truncated chunks; incorrect IFhd.
- Nested functions, mixed-width locals, stack/memory/local result destinations, saves inside output-filter continuations, heap fragmentation, and unordered MAll.
- Protection spanning RAM/extended memory during restore; failed restore preserving live state; independent RNG/I/O/string-table/Glk state after restore.
- Both outputs pushed onto the stack, overlapping output destinations, signed NaN conversion, ±0, Infinity tolerance, integer bounds, and mod quotients near integers.
- Save the same story in this interpreter and Glulxe, restore each save in the other, and compare subsequent output and return codes; retain the reference source revision and actual input record.
