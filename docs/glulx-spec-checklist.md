# Glulx Specification Compatibility Checklist

[English](glulx-spec-checklist.md) | [中文](glulx-spec-checklist.ZH.md)

Updated: 2026-09-08. The original checklist's main gaps were addressed in `4c16443`; acceleration functions, MOD, and inline text images in `c5fcc20`; windows, input, fonts, and styles in `4870bcf`; and media, streams, dates, and save edge cases in `5816d37`. This update adds previously unlisted player capabilities, optional extensions, and validation work. `[x]` means implementation and the listed validation are complete, not exhaustive conformance certification; new work uses `[ ]`. Specification evidence, classifications, and acceptance criteria are in the [remaining capability audit](glulx-remaining-spec-audit.md). Test details, fixture versions, and reproduction commands are in the [validation record](glulx-validation.md); implementation limits are in [compatibility](compatibility.md).

## Specification Boundaries

- [Glulx 3.1.3](https://eblong.com/zarf/glulx/Glulx-Spec.html): VM, instructions, saves, and capability queries.
- [Glk 0.7.6](https://eblong.com/zarf/glk/Glk-Spec-076.html): windows, streams, input, and optional media extensions.
- [Blorb 2.0.5](https://eblong.com/zarf/blorb/): executable and resource containers.
- [Glulxe](https://github.com/erkyrath/glulxe): differential and save interoperability reference. Source research remains in the [original research notes](glulx-spec-research.md).

## VM Core

Evidence: [VM](../src/vm.rs), [Memory](../src/memory.rs), [saves](../src/vm/save.rs), [regression matrix](../src/vm/conformance.rs).

- [x] Header, version, checksum, and ROM/RAM/extended-memory validation; `verify` checks the original executable image instead of always succeeding.
- [x] Big-endian access, ROM protection, memory growth/shrinkage, and overlapping copies; setmemsize/malloc return specification-defined failure on limits or reservation failure; stack overflow returns an error; zero-length mzero/mcopy do not access addresses, and function frames obey stack capacity.
- [x] Opcode encoding and standard addressing, narrow locals, integer/bit/array/search/stack operations and edge cases; dispatch and operand counts match all 150 official opcodes individually.
- [x] C0/C1, call/callf/tailcall, return, catch/throw, and call continuations.
- [x] Null/filter/Glk I/O, byte/Unicode/Huffman strings, indirect nodes, and calls with arguments; iterative output continuations, with 40,000 Huffman substring outputs matching the reference.
- [x] Complete single- and double-precision instruction families; conversions, NaN/Infinity, signed zero, tolerance, and double-word store order; branch operands are consumed even when not branching.
- [x] `save/restore`: IFZS identity, CMem/UMem, Stks, MAll, state preservation on failure, and protection ranges; bidirectional compatibility with Glulxe.
- [x] `accelfunc/accelparam` and Inform acceleration functions 1–13; unknown functions unregister and unknown parameters are legally ignored. Properties, classes, privacy, and old/new object layouts match Glulxe in differential tests.
- [x] Glulx gestalt matches implementation: 3.1.3, Float, Double, ExtUndo, and acceleration setup; functions 1–13 all advertise support.
- [x] Startup and `setrandom(0)` use system entropy; nonzero seeds are repeatable, with positive/negative/zero range regressions.
- [x] Multiple undo levels and correct hasundo/discardundo results; at most 16 states sharing a 64 MiB estimated budget.
- [x] restore/undo/restart do not roll back RNG, I/O system, string table, Glk objects, or protection definition; restart retains undo.
- [x] Heap allocation, coalescing, shrinkage on free, and save/undo ownership; regressions for the 256 MiB memory limit, failed allocation, and protection spanning extended memory.

## Glk

Evidence: [window tree](../src/vm/windows.rs), [streams](../src/vm/streams.rs), [events](../src/vm/events.rs), [presentation](../src/vm/presentation.rs), [sound](../src/vm/sound.rs), [GUI](../src/app.rs).

- [x] Pair window trees, parent/child/sibling relationships, arrangement queries/changes, nested layout, subtree closure, and resize/arrange events; arrangement changes do not reverse physical child order; blank/pair sizes are zero; text-window sizes use actual font metrics.
- [x] Fileref creation/selection/destruction, file/memory/resource streams, byte/Unicode char/line/buffer reads and writes, seek, counts, and echo streams; encoded-byte UTF-8 positioning/overwrite; closing streams removes echo bindings; file selection validates read paths and prompts before modifying existing files.
- [x] Per-window line/character requests, initial line content, cancellation results, multiple requests, queues, select/poll, timers, mouse, and hyperlinks; select_poll leaves player input untouched; cancellation retains edits; GUI validation covers grid inline input and special keys.
- [x] Latin1 input/case conversion and Unicode extended case/titlecase/NFC/NFD; official Unicode and resource-stream fixtures match reference output.
- [x] Style state, queries, and GUI text runs; paragraph indentation/hanging indents/four justification modes and text-buffer hints 0–9; grids support weight, oblique, colors/reverse colors while retaining fixed cell sizes. Queries report actual styles, and style commands propagate along echo chains.
- [x] Text-grid and graphics windows, actual window layout, image scaling/clipping, coordinates, and window-type capability parameters; text buffers support three inline alignments, either-side/repeated margin wrapping, flow-break, picture hyperlinks, and dynamic scaling. Zero-size images take no space. Graphics canvases clip/expand with windows and fill with the current background; rectangle dimensions are clipped as unsigned values.
- [x] Sound channels, play/repeat/stop/pause, volume/fades, completion notifications, and multi-channel playback; pure Rust on-demand MOD/XM/S3M/IT decoding; play_multi starts at the same stereo sample frame. Sound is not advertised without an audio device.
- [x] Date/time, resource streams, line terminators, and echo control; UTC/local, negative-time, and date-normalization regressions; Gregorian arithmetic across the full i32 year field, with isolated timezone tests for DST gaps, skipped days, and ancient/future offsets.
- [x] Dispatch references/arrays/structures/stack results, object lifetimes, and output parameters; all 124 official selectors dispatch; unknown selectors are recorded and return 0.
- [x] Gestalt checked per argument; GUI CharOutput reflects real font coverage with system/custom fonts; headless does not advertise GUI graphics, mouse, sound, or similar capabilities.
- [x] Upgrade to Glk 0.7.6 with `image_draw_scaled_ext`; graphics draws retain call-time dimensions; text buffers reflow with current window width, supporting proportional/aspect/maxwidth rules and transparent images.

## Blorb and Player

- [x] FORM/IFRS chunk boundaries and RIdx validation, indexed executables, picture/sound/data resources, and FORM audio resource headers.
- [x] iFiction metadata, Fspc cover art, RDes image/sound descriptions, and the story information panel.
- [x] GUI session saving every 30 seconds and on normal exit; startup without a story restores VM, Glk, pending input, graphics canvases, timers, and audio progress. Desktop snapshots are separate from portable IFZS.

## Validation and Maintenance

- [x] 181 Rust tests (179 library and 2 CLI) pass; domain-specific matrices and edge-case coverage are in the validation record.
- [x] Final integrated Glulxercise general, single-precision, and double-precision runs yielded 92 passing sections, passing all three rounds; an earlier random-distribution threshold failure is also retained in the validation record.
- [x] All 94 results for Inform acceleration functions 1–13 exactly match Glulxe; the synthetic media story verifies image reflow, clicks, MOD completion events, and session restoration.
- [x] Pinned Glulxe/CheapGlk revisions, synthetic-story and Adventure bidirectional save validation, and exact normalized Unicode/resource-stream output comparison.
- [x] Public-story regressions expanded to Adventure, Unicode, resource streams, input extensions, date/time, multiple windows, and Sensory Jam; focused tests, startup smoke tests, and GUI interactions are clearly distinguished.

## Subsequent Audit Additions

- [x] Consistent reads/writes across streams for the same file, preventing older handles from overwriting new data on close.
- [x] Timestamp conversion beyond chrono's range but within Glk's date fields.
- [x] Specification handling of repeated IFZS ANNO/unknown chunks and duplicate known chunks.
- [x] Exact finite audio repetition; XM/S3M/IT inside Blorb MOD resources; packet-based AIFF/OGG/MP3 decoding, encoder-padding trimming, and continuous resampling.
- [x] Failed draws for corrupt images, bounded clipped sampling for oversized destinations, and GPU texture-dimension adaptation.
- [x] Blorb RIdx first-chunk rule and RDes text descriptions.

## Completed Player Additions

- [x] Separate Blorb resource attachment: specify an archive without an executable for raw `.ulx`, with public API, CLI arguments, and GUI entry points; images, sound, Data, and descriptions use the selected archive. Validate an existing `IFhd` against the first 128 Glulx bytes; allow absence; wrong identity/corrupt archives must not replace existing resources.
- [x] Blorb identity and argument-conflict diagnostics: embedded-executable IFhd matches the story; an explicit separate story plus an archive containing Exec reports a conflict; do not mix in the Z-machine identity layout.
- [x] Automatic same-name resource discovery (player policy): search the story directory for same-name Blorbs, document candidate priority and explicit-selection precedence, and provide understandable outcomes for absent, corrupt, or mismatched candidates.
- [x] Session restoration with separate resources: retain resource origin and content so images, sound, and Data remain available after restoration; cover both bundled and external resources.
- [x] Legacy `SONG` audio format: parse `SND<number>` external AIFF sample references and sustain loops, integrating existing playback, repetition, pause, and restoration paths. Blorb explicitly marks this extension optional and deprecated.
- [x] Real terminal host: display successfully created text-grid/status windows and multi-window layouts; support prefilled editing, timed cancellation returning current edits, and immediate unechoed character input without Enter; preserve pipe-driven automation and accurately distinguish its capabilities.
- [x] Actual light font weight (optional display enhancement): select and render a light face when available, aligning style_measure/style_distinguish with actual presentation; accurately report fallback values when no font is available.

- [x] Loose resource directory (optional player convenience): explicit directory selection and documented PIC/SND/DATA names/formats, covering number parsing, types, path boundaries, and session restoration; Glk permits omitting this entry point.

## Validation Still Required

- [ ] Windows on-device validation: windows/fonts/DPI, input/special keys, file prompts/paths, audio, normal exit, and session restoration; record OS and build versions.
- [ ] macOS on-device validation: windows/fonts/Retina, input/special keys, file prompts/paths, audio, normal exit, and session restoration; record OS and build versions.
- [ ] Complete long-game walkthroughs: pin game versions and retain reproducible routes covering cross-chapter state, game save/load, undo/restart, and continuation after closing the player mid-game.
- [ ] Expanded media fixture matrix: PNG/JPEG variants, sampled-audio bit depths/sample rates/channels, and historical tracker variants; record support and failure behavior individually.
- [ ] Physical audio-output validation: relate synchronized starts, pause/resume, fades, and completion notifications to actual output, adding evidence beyond software sample-frame checks.

Separate resources, terminal input and the listed optional player additions are implemented and have local validation. Cross-platform validation remains pending. Validation tasks above do not imply known missing functionality; existing passing records remain valid.

## Current Limits and Specification Scope

Grids retain uniform cell sizes as specified, ignoring hints 0–3/6 that change cell layout; available light faces render at light weight; missing faces fall back to regular and report that accurately. Missing glyphs return CannotPrint, and fallback fonts can be loaded. MOD/XM/S3M/IT are supported without claiming every historical dialect or bit-exact playback against specific hardware. Blorb Rect/Reso/APal/Loop are in Z-machine scope. VM memory is limited to 256 MiB; undo retains at most 16 states sharing a 64 MiB estimated budget. Image sources above 16 megapixels or the decoder's 128 MiB allocation budget are unavailable; draw destination dimensions are not subject to this source-image limit. These resource limits are current implementation policy; undo estimates include memory/stack/story image but exclude heap indexes and allocator overhead, and are not marked as missing features. Container reading still tolerates nonzero padding and unindexed GLUL fallback; see the audit. Rejection of every invalid container is not claimed.
