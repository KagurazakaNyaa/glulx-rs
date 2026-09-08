# Public Glulx Specification Sources and Review Notes

[English](glulx-spec-research.md) | [中文](glulx-spec-research.ZH.md)

Researched and reviewed: 2026-09-08. The official Glulx/Glk/Blorb home pages and the complete Glulx 3.1.3, Glk 0.7.6, and Blorb 2.0.5 specifications were read. For implementation status, see the [compatibility checklist](glulx-spec-checklist.md). This document records specification evidence so implementation plans or old documentation are not mistaken for the specification. Gaps and validation requirements after `5816d37` are covered in the [remaining capability audit](glulx-remaining-spec-audit.md).

## Correct Entry Points and Versions

| Scope | Official sources | Verified findings |
| --- | --- | --- |
| Glulx VM | [Home](https://eblong.com/zarf/glulx/), [HTML](https://eblong.com/zarf/glulx/Glulx-Spec.html), [Markdown](https://eblong.com/zarf/glulx/Glulx-Spec.md) | Current version: 3.1.3 |
| Glk I/O | [Home](https://eblong.com/zarf/glk/), [HTML](https://eblong.com/zarf/glk/Glk-Spec-076.html), [Markdown](https://eblong.com/zarf/glk/Glk-Spec-076.md) | Current version: 0.7.6; this repository advertises and implements 0.7.6 |
| Blorb container | [Home](https://eblong.com/zarf/blorb/), [specification](https://eblong.com/zarf/blorb/Blorb-Spec.html) | Version 2.0.5; subsequent review covered resource archives, IFhd, media formats, and optional features in the full text |

Note: the [old lowercase URL](https://eblong.com/zarf/glulx/glulx-spec.html) remains accessible, but returned the paginated **3.1.2** specification during this review. New documentation should use the correctly capitalized entry points above to avoid missing double precision and extended undo.

## Specification Requirements for the Checklist

| Specification section | Implementation/validation implications |
| --- | --- |
| [The Machine](https://eblong.com/zarf/glulx/Glulx-Spec.html) | Check the memory map, header, stack frames, call stubs, instruction encoding, and object types; counting opcodes alone is insufficient. |
| [Save-Game Format](https://eblong.com/zarf/glulx/Glulx-Spec.html#saveformat) | Saves are a Quetzal variant: CMem/UMem include the current memory size and save RAMSTART through the current memory end; the stack includes continuations, MAll saves the heap, and IFhd uses the first 128 bytes. |
| [State Not Saved, in the save section](https://eblong.com/zarf/glulx/Glulx-Spec.html#saveformat) | Glk state, protection definition, internal RNG state, I/O system, and string-table address do not roll back on restart/restore/restoreundo. The current implementation and regressions observe this state boundary. |
| [Random Number Generator](https://eblong.com/zarf/glulx/Glulx-Spec.html#opcodes_rand) | Nonzero seeds must produce repeatable sequences; seed=0 and initial startup use nondeterministic mode; RNG is not saved state. |
| [Accelerated Functions](https://eblong.com/zarf/glulx/Glulx-Spec.html#opcodes_accel) | Implementations may provide no concrete acceleration functions. An unknown function number unregisters acceleration at that address; unknown parameter settings are ignored. Distinguish setup instructions from actual optimized functions. |
| [Gestalt / Miscellaneous](https://eblong.com/zarf/glulx/Glulx-Spec.html#opcodes_misc) | Acceleration(9) must be true for 3.1.1+; AccelFunc(10) queries a specific function; Float(11), ExtUndo(12), and Double(13) query their respective capabilities. Glulx Unicode does not replace Glk Unicode detection. |
| [Double-Precision Math](https://eblong.com/zarf/glulx/Glulx-Spec.html#opcodes_double), [Comparisons](https://eblong.com/zarf/glulx/Glulx-Spec.html#opcodes_doublebranch) | 3.1.3 adds double precision; interpreters may omit it but must report Double accurately. Defined instructions lie in 0x200–0x239, with gaps. |
| [Game State, in the instruction text](https://eblong.com/zarf/glulx/Glulx-Spec.html) | hasundo/discardundo are 3.1.3 extensions queried through ExtUndo. verify checks file length/checksum; the specification permits automatic verification at startup. |
| [Glk 0.7.6 specification](https://eblong.com/zarf/glk/Glk-Spec-076.html) | Glk is a compatibility layer independent of the VM, covering windows, streams, events, characters, images, sound, and optional modules. 0.7.6 adds image_draw_scaled_ext and changes oversized-image fitting in text windows. The repository has completed this upgrade; test evidence is in the validation record. |

## Implementation Review Method

Map each requirement to `operand_count/step/gestalt/glk/glk_gestalt` in [vm.rs](../src/vm.rs), [memory.rs](../src/memory.rs), [story.rs](../src/story.rs), and [app.rs](../src/app.rs). Record implemented branches, constant returns, no-op placeholders, and fully usable capabilities separately. Passing tests proves only the cases executed, not every specification boundary.

Behavioral differential testing can use the officially linked [Glulxe reference implementation](https://github.com/erkyrath/glulxe) and the project's existing reference, [Git](https://github.com/DavidKinder/Git). The initial research did not run interpreters; subsequent Glulxe differential testing and bidirectional save interoperability are complete. Versions and results are in the [validation record](glulx-validation.md).
