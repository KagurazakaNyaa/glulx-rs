# Audit of Remaining Glulx Specification Items

[English](glulx-remaining-spec-audit.md) | [中文](glulx-remaining-spec-audit.ZH.md)

This document preserves the pre-implementation audit at `5816d37`, committed in `0a6d5b4`. Its unchecked boxes are original acceptance proposals, not the current task status. The implemented follow-up and actual validation are tracked in the [checklist](glulx-spec-checklist.md) and [validation record](glulx-validation.md).

Date: 2026-09-08. Code baseline: `5816d37`. This round reviews specifications and updates documentation before implementation. It records differences between primary sources read and current code; tasks are not completion records. Coverage includes the Glulx 3.1.3 core, Blorb 2.0.5 resources and sound formats, and Glk 0.7.6 host input/presentation. Both confirmed gaps and boundaries with no newly identified gaps are recorded below.

## Primary Sources and Classification

| Source | Scope actually reviewed |
| --- | --- |
| [Blorb 2.0.5 specification](https://eblong.com/zarf/blorb/Blorb-Spec.md), maintained by [IFTF](https://github.com/iftechfoundation/ifarchive-if-specs) | Overall Structure, Resource Index, Picture/Sound/Data/Executable Resource Chunks, Game Identifier, Fspc, RDes, Z-machine-specific chunks, IFF, Other Resource Arrangements |
| [Glk 0.7.6 specification](https://eblong.com/zarf/glk/Glk-Spec-076.md) | [Resource Streams](https://eblong.com/zarf/glk/Glk-Spec-076.html#resource_streams), [Sound Resources](https://eblong.com/zarf/glk/Glk-Spec-076.html#sound_resources), [Playing Sounds](https://eblong.com/zarf/glk/Glk-Spec-076.html#sound_playing), [Blorb Layer](https://eblong.com/zarf/glk/Glk-Spec-076.html#blorblayer) |
| [Glulx 3.1.3 specification](https://eblong.com/zarf/glulx/Glulx-Spec.md) | Save-Game Format → Associated Story File: `IFhd` is the first 128 ROM bytes |
| [Original Apple AIFF-C document](https://eblong.com/zarf/ftp/aiff-c.9.26.91.ps), linked directly by Blorb's AIFF Sounds section | §6 Marker Chunk and §9 Instrument Chunk: sample-frame marker positions, `sustainLoop` modes and endpoints. The document identifies itself as a 1991-08-26 draft; its SAXEL proposal is outside this project's targets. |

The first three specifications were reviewed using local copies downloaded for this task. SHA-256 hashes are listed below, together with the AIFF-C document:

```text
Blorb-Spec.md     ce642b0875a4ed61e199b3f0d1c0e54f22171bdbb6f26cd518de5f4cc5c41b4d
Glk-Spec-076.md   974d4c63539521a6e57efa967418e3dde463f53e45871de8abaed961b9003eea
Glulx-Spec.md     ccd5e8aacff3cbd7906e5fcac055d7421dd9d9297634a9c86eff54f5405f36e7
aiff-c.9.26.91.ps e7a905a06cd8b60b67ac45a7f66bffe7f1f9e9fc8ba31d68f2639f46a36ea524
```

“Specification requirement” means a format or behavioral rule once a capability is supported; Glk graphics, sound, and similar modules may themselves be omitted. “Optional extension” means a feature the specification explicitly permits omitting. “Player extension” means host behavior such as entry points, discovery policy, and desktop sessions. “Validation gap” means existing functionality lacks specified acceptance evidence and must not be labeled unimplemented.

## Separate Blorb Archives and Story Identity

| Item | Specification and current evidence | Classification and work required |
| --- | --- | --- |
| Resource archives without executables | Blorb's “Executable Resource Chunks” explicitly permits archives without Exec alongside a separate executable; an archive alone cannot run. [Story::from_bytes](../src/story.rs) calls `extract_glul_chunk` for every FORM, raw `.ulx` has an empty resource index, and no attachment API exists. [CLI](../src/main.rs) and [GUI](../src/app.rs) load only one story path. | **Missing valid Blorb usage.** Add separate resource parsing/attachment and API/CLI/GUI entry points; missing Exec must not make resource attachment fail. Do not execute resource-only archives as stories. |
| Argument conflicts | The same section calls for diagnostics when a separate executable is supplied while the resource archive also contains one. | **Specification-recommended diagnostic.** Reject or warn about this combination at attachment; never silently replace the explicitly selected story. Any override mode must be identified as player policy. |
| `IFhd` matching | Blorb's “The Game Identifier Chunk” makes IFhd optional; when present, it can identify the associated story and a mismatch should error. Embedded executables should also match IFhd. Glulx “Associated Story File” defines the first **128 bytes**, including length, checksum, and compiler data. [story.rs](../src/story.rs) does not check Blorb IFhd; [save.rs](../src/vm/save.rs) uses 128 bytes for IFZS, which does not prove the Blorb path checks it. | **Missing recommended consistency check.** Allow attachment without IFhd; otherwise check length/content, covering matches, mismatches, truncation, and embedded executables. Do not use the Z-machine's 13-byte release/serial/checksum/Initial PC layout. |
| Unified resource mapping | The resource file registered in Glk's Blorb Layer supplies resource lookups. [Story](../src/story.rs) reads pictures, audio, Data, Fspc, and RDes from the current container/index. | **Implementation boundary.** The selected archive must apply to all these entry points while retaining the execution image and VM identity. Validate before replacement; failed attachment preserves state. |
| Resource origin and sessions | [session.rs](../src/vm/session.rs) rebuilds Story from `container.unwrap_or(image)`, assuming the container includes an executable; serde skips the resource index. | **Missing player feature.** Save separate resource origin and content; restore by validating executable and archive independently and rebuilding indexes. Do not rediscover a different same-name file during restore. Portable IFZS remains responsible only for specified VM state. |
| Same-name discovery | Blorb “File Suffixes” defines `.blorb`, `.gblorb` containing Glulx, and short suffixes, but no same-name discovery algorithm. Glk “What the Program Does” delegates startup file discovery to the host. | **Player extension.** Document suffix/case/multiple-candidate priority, explicit-selection precedence, corrupt-archive diagnostics, and no-candidate behavior for raw `.ulx`. Do not label it a missing Glk opcode. |

Glk explicitly says the Blorb Layer “is not part of the Glk API per se”. `giblorb_set_resource_map` integrates a C host with the resource library. A Rust player needs equivalent resource mapping, not an invented standard Glulx dispatch selector or a change to its Rust dependencies merely to use this format. [Reference](https://eblong.com/zarf/glk/Glk-Spec-076.html#blorblayer)

Separate-resource acceptance checks to add:

- [ ] Split pinned `resstreamtest.gblorb` into `.ulx` and an archive without Exec; compare Data/Unicode output against the original archive using the existing reference transcript.
- [ ] Provide PNG/JPEG, AIFF/MOD, TEXT/BINA/FORM, Fspc, and RDes in a synthetic archive, checking all calls use the selected mapping; FORM resources retain their own eight-byte headers.
- [ ] Cover absent IFhd, matching 128-byte IFhd, incorrect lengths/content, corrupt FORM/RIdx, duplicate resource numbers, and argument conflicts; verify consistent story/resources after failure.
- [ ] Explicit CLI arguments in GUI/headless modes and GUI selection; same-name discovery priorities/errors; no external resources carried over when switching stories.
- [ ] Save a desktop session, move or delete original resource files, then restore with images, Data, and descriptions still readable and sound continuing from recorded progress; cover embedded archives, separate archives, and old sessions.

If resources can be replaced while running, define behavior for open resource streams, playing sounds, and already drawn images, and clear the [VM image-size cache](../src/vm/presentation.rs) and [GUI image/cover caches](../src/app.rs). This is dynamic-replacement acceptance work and cannot be checked off using startup-only attachment tests.

## SONG: Optional Format and Rules Required Once Implemented

Blorb “Song Sounds” states: “The song file format is deprecated, as of Blorb 2.0.” and “Its support in interpreters should be considered optional.” Missing `SONG` therefore is not a missing required Glulx instruction and does not negate MOD/XM/S3M/IT support. Currently [sound.rs](../src/vm/sound.rs) sends only `MOD ` to the tracker, and other formats to sampled decoding; [tracker.rs](../src/vm/sound/tracker.rs) has no resource-resolution callback, SND references, or AIFF sustain-loop assembly. The existing [validation record](glulx-validation.md) has no SONG fixture. [Specification](https://eblong.com/zarf/blorb/Blorb-Spec.md)

If SONG is implemented, the following format rules and acceptance checks cannot be replaced by ordinary MOD playback:

- [ ] Parse `SND<number>` in the original MOD's 22-byte sample-name fields and resolve AIFF through the same resource mapping. Targets must be AIFF, not MOD or another SONG. Cover shared samples, different IDs, missing resources, wrong types, truncated names/files, and number overflow. Blorb does not define recursive references between SONGs.
- [ ] Ignore SONG's own sample length, repeat start, and repeat length; use AIFF sample length and instrument `sustainLoop`, retaining the MOD sample record's finetune and volume. Test deliberately incorrect old length/loop fields so accidentally matching fixtures cannot hide omissions.
- [ ] Without INST, or with `sustainLoop.playMode == NoLooping`, treat repeat start/length as zero; releaseLoop does not replace sustainLoop. Reference: Blorb “Song Sounds”.
- [ ] Parse loops through AIFF MARK/INST: `beginLoop/endLoop` are **marker IDs** mapped to **sample-frame positions**, not direct byte offsets. Cover NoLooping(0), ForwardLooping(1), and ForwardBackwardLooping(2). Ignore loops with begin greater than or equal to end, as AIFF specifies; corrupt/missing markers need bounded, explicit failure or fallback behavior. [AIFF §6/§9](https://eblong.com/zarf/ftp/aiff-c.9.26.91.ps)
- [ ] Use AIFF with nonzero SSND offset and different valid bit depths/sample lengths, checking assembled pitch, volume, and loop boundaries; record conversion policy for stereo and other multi-channel samples. Blorb explicitly permits trimming/padding AIFF samples to 8 bits to assemble MOD and does not require high-bit-depth preservation; document this honestly if used. [Blorb AIFF/Song Sounds](https://eblong.com/zarf/blorb/Blorb-Spec.md)
- [ ] Integrate Glk playback returns, 0/1/finite/infinite repeats, pause/resume, stop/replacement, completion notifications, and play_multi. Notify only after the last finite repetition, never for zero repeats, infinite playback, or interruption. Desktop audio restoration offsets require separate player validation. [Glk Playing Sounds](https://eblong.com/zarf/glk/Glk-Spec-076.html#sound_playing)
- [ ] Compare PCM/frame counts and repetition boundaries for synthetic SONG and an equivalent assembled module; include GUI playback and desktop restoration validation. Use a finite sampling budget to check malicious lengths cannot cause unbounded allocation.

Blorb's `Loop` chunk gives whole-sound repetition hints in Z-machine scope; it cannot replace the referenced AIFF's `INST.sustainLoop`. [Blorb “Chunks Specific to the Z-machine”](https://eblong.com/zarf/blorb/Blorb-Spec.md)

## Implemented Features, Permitted Omissions, and Validation Gaps

| Item | Review finding | Sources and code |
| --- | --- | --- |
| ResourceStream | Read-only TEXT/BINA/FORM by Data number, Latin-1/UTF-8/big-endian words, FORM headers, and LF termination already exist, with exact official-fixture transcript comparison. Separate archive attachment does not require rewriting these interfaces. | [Glk Resource Streams](https://eblong.com/zarf/glk/Glk-Spec-076.html#resource_streams); [streams.rs](../src/vm/streams.rs), [Story::resource_file](../src/story.rs), [validation](glulx-validation.md) |
| Loose `PIC1/SND1/DATA1` files | Unimplemented. Glk says “may”; Blorb explicitly calls this a platform-specific arrangement. It is another optional development convenience, not resource-stream nonconformance for a Blorb-only implementation. | [Glk Resource Streams](https://eblong.com/zarf/glk/Glk-Spec-076.html#resource_streams), [Blorb Other Resource Arrangements](https://eblong.com/zarf/blorb/Blorb-Spec.md); [streams.rs](../src/vm/streams.rs) |
| `glk_sound_load_hint` | A no-op is permitted. This only affects optional preloading, not playback semantics, and should not be listed as missing functionality. | [Glk sound section](https://eblong.com/zarf/glk/Glk-Spec-076.md), `glk_sound_load_hint` definition; [sound.rs](../src/vm/sound.rs) |
| Standard sampled/tracker formats | Blorb defines AIFF, OGGV, MP3, and MOD/XM/S3M/IT sharing the `MOD ` tag. Existing decoders and synthetic tests cover them. SONG is a separate optional item. | [Blorb Sound Resource Chunks](https://eblong.com/zarf/blorb/Blorb-Spec.md); [sound.rs](../src/vm/sound.rs), [tracker.rs](../src/vm/sound/tracker.rs), [sampled.rs](../src/vm/sound/sampled.rs) |
| Format variants and sound-card output | Basic support for all categories does not validate every encoding bit depth/sample rate/channel count/historical tracker variant. Software sample synchronization cannot replace physical sound-card waveform checks; these remain validation tasks. | [Blorb MOD Sounds](https://eblong.com/zarf/blorb/Blorb-Spec.md) does not exhaustively list internal dialects of the four trackers; [validation matrix and scope](glulx-validation.md) |
| `Plte` and Z-machine hints | Plte may be ignored entirely. RelN/Reso/APal/Loop are defined for Z-code; Rect is explicitly optional with undefined Glulx behavior. These are not required features for this Glulx player. Existing Z-machine scope documentation could also list RelN. | [Blorb Color Palette, Placeholder Pictures, Chunks Specific to the Z-machine](https://eblong.com/zarf/blorb/Blorb-Spec.md) |
| Fspc, RDes, IFmd | Cover art, resource descriptions, and metadata display already exist. RDes is not required; separate-archive validation should cover these same capabilities rather than relisting them as unimplemented. | [Blorb Frontispiece, Resource Description, Metadata](https://eblong.com/zarf/blorb/Blorb-Spec.md); [story.rs](../src/story.rs), [app.rs](../src/app.rs) |

The existing checklist's “strict FORM/IFRS chunk boundaries and RIdx validation” covers boundaries, RIdx first/unique placement, index lengths/offsets, and duplicate IDs. Review found [blorb_chunks](../src/story.rs) skips odd-length chunk padding without checking it is zero, and `extract_glul_chunk` permits an unindexed GLUL compatibility fallback. Blorb's IFF rules require generated padding to be zero and its Resource Index rules require indexed resources, but do not require interpreters to reject all invalid inputs. Record this as **permissive reading policy/validation coverage limits**, not missing valid-game functionality. Continued claims of fully strict container validation require corresponding checks and corrupt fixtures. [Specification](https://eblong.com/zarf/blorb/Blorb-Spec.md)

This round only read specifications and inspected code; it did not rerun existing tests or mark the pending checks above as passed. Evidence for the existing 158 tests and reference/GUI results remains the [previous validation record](glulx-validation.md).

## Glk Terminal Host and Fonts

| Item | Specification and implementation evidence | Tasks and acceptance criteria |
| --- | --- | --- |
| Terminal grid/status display | [Text Grid Windows](https://eblong.com/zarf/glk/Glk-Spec-076.html#window_textgrid) defines visible grids and inline input. The headless loop in [main.rs](../src/main.rs) only prints take_output, not grid/status/window_views, while [windows.rs](../src/vm/windows.rs) still permits grids. | **Real terminal host gap.** Use PTY checks for visibility of successfully created windows, grid updates/closure, and multi-window layout. State pipe-mode scope accurately; do not claim complete terminal presentation. |
| Terminal line/character input | [Line Input Events](https://eblong.com/zarf/glk/Glk-Spec-076.html#line_events) requires treating prefill as entered input and returning current composition on cancellation. [Text Buffer Windows](https://eblong.com/zarf/glk/Glk-Spec-076.html#window_textbuf) specifies unechoed character input. Current main uses stdin.read_line without showing initial_input or continuously calling update_line_input; character requests also require Enter. | **Input host semantics gap.** On a real TTY, cover prefilled editing, timed cancellation during input, immediate single-key delivery, and no echo. Retain pipe transcript regressions and validate EOF/terminal restoration. |
| Light font weight | [Style Hints](https://eblong.com/zarf/glk/Glk-Spec-076.html#stream_style_hints) explicitly permits ignoring hints. [presentation.rs](../src/vm/presentation.rs) falls back from light to regular and accurately returns 0; [fonts.rs](../src/app/fonts.rs) currently filters only regular faces. | **Optional display enhancement.** Load a real light face and verify rendering, style_measure=-1, and style_distinguish. Report actual fallback values without a matching font. Current legal fallback is not an unmet mandatory Glk requirement. |
| Style measurement units | [Testing Styles](https://eblong.com/zarf/glk/Glk-Spec-076.html#stream_style_check) permits platform-defined indentation/font-size units and queries actual appearance. Current pixel indentation/actual font-size returns and headless inability to measure/distinguish fit that scope. | No new gap identified. |
| Media capabilities | [Sound Capabilities](https://eblong.com/zarf/glk/Glk-Spec-076.html#sound_testing), [Hyperlink Capabilities](https://eblong.com/zarf/glk/Glk-Spec-076.html#link_testing); [vm.rs](../src/vm.rs) reports by host, window type, and audio device. Hyperlinks means function availability, separate from per-window HyperlinkInput. | No new confirmed gap; do not misreport truthful lack of support without a device as an implementation omission. |

## Glulx Core and Documentation Precision

A bounded recheck of call/output continuations, Null/filter/Glk I/O, Float/Double, IFZS, heap/undo, and acceleration found no new confirmed missing feature or semantic error for valid programs. This conclusion comes from specification/code comparison, not from treating 150-entry dispatch completeness as semantic evidence; this review still does not prove full conformance.

- `setmemsize/malloc` have specified failure paths for limits and reservation failures in [memory.rs](../src/memory.rs); this cannot be generalized to recovery from all Rust allocation/OOM failures. Stack overflow is a VM error. [Memory instructions](https://eblong.com/zarf/glulx/Glulx-Spec.html#opcodes_memory), [malloc](https://eblong.com/zarf/glulx/Glulx-Spec.html#opcodes_malloc).
- Undo's 64 MiB is estimated accounting of memory/stack/image in [vm.rs](../src/vm.rs), excluding heap_blocks indexes and allocator overhead; call it an estimated budget. [Saved state](https://eblong.com/zarf/glulx/Glulx-Spec.html#saveformat). These are documentation precision corrections, not new core implementation tasks.

## Validation Boundaries and Implementation Order

Windows/macOS hardware, complete long-game routes, more media encoding variants, and physical audio output remain validation tasks. The existing [release workflow](../.github/workflows/release.yml) configures three-platform tests/builds; its existence does not mean it ran this version, much less establish GUI hardware validation. Record actual platforms, builds, input routes, and results, updating checklist items individually.

Commit this documentation first; then implement separate resource attachment/identity/session support, a real terminal host, SONG, and actual light font weight. Loose resource discovery is an optional player convenience tracked separately, not a Glk standard selector. Update status after implementation using the acceptance criteria above; platforms and scenarios not run remain unchecked.
