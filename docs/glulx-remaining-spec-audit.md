# Current Glulx Specification Audit

[English](glulx-remaining-spec-audit.md) | [中文](glulx-remaining-spec-audit.ZH.md)

Updated: 2026-09-10. This audit describes the current implementation state at commit `f9c855d`. It is a current scope and risk record, not a historical implementation log. Detailed test commands are in the [validation record](glulx-validation.md), and implementation limits are in [compatibility](compatibility.md).

## Sources and Classification

The review uses the [Glulx 3.1.3 specification](https://eblong.com/zarf/glulx/Glulx-Spec.html), [Glk 0.7.6 specification](https://eblong.com/zarf/glk/Glk-Spec-076.html), [Blorb 2.0.5 specification](https://eblong.com/zarf/blorb/Blorb-Spec.md), and the [AIFF-C document](https://eblong.com/zarf/ftp/aiff-c.9.26.91.ps) referenced by Blorb. A specification requirement applies when the corresponding capability is provided. Optional Glk modules and optional Blorb formats are not mandatory conformance gaps.

## Current Status

| Area | Current implementation | Remaining boundary |
| --- | --- | --- |
| Glulx core | Header/checksum validation, official opcode dispatch, integer/single/double precision, strings, heap, acceleration, IFZS, restart/protect, and undo are implemented and covered by focused regressions. | Full conformance across every valid/invalid program and every host combination is not proven by table or unit-test coverage alone. |
| Resource-only Blorb | External resource-only Blorbs and loose resource directories can be attached to raw stories through the API, CLI, GUI, and discovery policy. IFhd identity, conflicts, resource indexes, session retention, and cache reset are covered. | Runtime resource replacement is intentionally a restart workflow; behavior for already-open resource streams or currently playing sounds is not a separate live-swap API. |
| Resource streams | TEXT/BINA/FORM streams, encoded positions, Unicode, metadata, cover art, RDes, and resource precedence are implemented. | The loader is intentionally permissive about some invalid-container cases described below. |
| SONG | Optional SONG support resolves `SND<number>` AIFF samples, SSND offsets, MARK/INST sustain loops, sample conversion, repeats, offsets, pause/stop, notifications, and session restoration. | All historical encoder variants and physical audio output are not claimed. |
| Terminal host | Interactive TTY mode supports grids/status, prefilled editing, timed cancellation, immediate character input, echo/terminators, file prompts, multi-window selection, and terminal restoration. Pipe mode remains a stable automation protocol. | Windows console and macOS on-device terminal validation remain open. |
| Light weight | Real light font faces are used when available; missing faces fall back to regular and report the actual capability. | Font/DPI behavior on Windows and macOS remains unverified. |
| Undo budget | Retained page snapshots, stack bytes, and heap-record payloads are charged; the shared story image and current VM address space are not charged. Zero disables retention; oversized candidates are rejected before the complete page table is materialized. | Page-count estimation and page construction still deserve profiling on very large dirty sets. |
| VM memory layout | `Memory` stores only `RAMSTART..current_end`; ROM reads come from the shared story image, and bundled Blorb container/image storage shares one backing buffer. | Cross-boundary reads/copies and legacy desktop-session migration are covered; mmap and lazy container access remain future work. |
| Container strictness | FORM/IFRS boundaries, RIdx structure, resource offsets, duplicate IDs, and identity checks are validated. | Odd padding bytes and an unindexed GLUL compatibility fallback are tolerated. The player does not claim rejection of every malformed container. |

## Optional and Host-Specific Scope

- `glk_sound_load_hint` may be a no-op because it only requests optional preloading.
- Blorb `Plte`, Z-machine-specific chunks, and unsupported historical tracker dialects are outside the Glulx player target unless a host feature explicitly adopts them.
- Loose resource directories are a player convenience, not a required Glk dispatch selector.
- Physical sound-card waveform validation, long-game walkthroughs, and cross-platform GUI behavior require platform-specific evidence and are not inferred from Linux or software-sample tests.

## Validation Priorities

1. Run the current Linux reference, GUI, and TTY scripts from the [validation record](glulx-validation.md) when changing VM, resource, or host behavior.
2. Add Windows and macOS on-device records for GUI, fonts/DPI, terminal input, file prompts, audio, and session restoration.
3. Expand the media matrix with codec, bit-depth, sample-rate, channel-count, and historical tracker variants; record failure behavior as well as successful playback.
4. Extend the current headless profile to a longer scripted route with images/audio, then choose between mmap, block compilation, and further UI changes from measured costs.

These priorities are engineering and validation work, not claims that the current implementation violates a mandatory Glulx or Glk rule.
