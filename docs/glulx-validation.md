# Glulx Implementation Validation Record

[English](glulx-validation.md) | [中文](glulx-validation.ZH.md)

Date: 2026-09-08, Linux x86_64. Core implementation commit `4c16443`, acceleration/media commit `c5fcc20`, and input/font/window commit `4870bcf`. Subsequent work completed fixes for media formats, shared files, wide-range dates, IFZS, and image edge cases.

The remaining-feature audit was committed as `0a6d5b4` before implementation; bilingual documentation was committed separately as `af210ba`. Separate resources, SONG, interactive terminal input and actual light weight now have the local validation recorded below. Unexecuted platform and broader scenario checks remain open in the [specification checklist](glulx-spec-checklist.md).

## Reproduction Commands and Results

```sh
RUSTC_WRAPPER= cargo fmt --all -- --check
RUSTC_WRAPPER= cargo test --all-targets
RUSTC_WRAPPER= cargo clippy --all-targets -- -D warnings
RUSTC_WRAPPER= cargo build --release
python3 tools/check-opcodes.py --spec /path/to/Glulx-Spec.md --output /tmp/glulx-opcode-audit.tsv
python3 tools/check-reference.py --reference /path/to/glulxe --candidate target/debug/glulx-rs --fixtures /path/to/fixtures
```

Rust tests: 180 passed / 0 failed (178 library, 2 CLI); Clippy with warnings as errors and the release build passed. Scripts do not download fixtures or modify repository game resources; synthetic stories and saves use temporary directories. Without `--fixtures`, synthetic IFZS bidirectional interoperability, double stack order, Inform acceleration, zero-length memory, and deep-string differential checks still run. Dispatch and operand counts match 150/150 official opcodes; all 124/124 official Glk dispatch selectors are present. These two checks establish table completeness only; execution semantics still require runtime tests.

The final integrated full script passed with:

```text
PASS Rust -> Glulxe: save continuation, heap chunk, double stack order
PASS Glulxe -> Rust: save continuation, heap chunk, double stack order
PASS acceleration: all 13 functions, 94 result checks, exact reference transcript
PASS core boundaries: zero-length memory operations and 40000 Huffman substrings, exact reference transcript
PASS shared file streams: cross-handle reads, independent counts, and final file bytes match reference
PASS glulxercise.ulx: 92 passing sections
PASS unicasetest.ulx: exact normalized reference transcript
PASS resstreamtest.gblorb: exact normalized reference transcript
PASS Adventure Rust -> Glulxe
PASS Adventure Glulxe -> Rust
```

Glulxercise received `all / allfloat / alldouble / quit` and reported `All tests passed.` three times. Its random-distribution test can produce statistical false positives, as the official fixture explicitly notes; a single statistical failure should be recorded and analyzed, not rerun to conceal a deterministic defect. In one rerun before `4870bcf`, the random group produced lobit=141 / hibit=99 across 240 samples, outside the fixture's [100..140] threshold, failing two assertions in that group. Other groups and allfloat/alldouble passed. This run is recorded as a statistical threshold failure, not a full-suite pass. The necessary integration regression after shared-stream and IFZS changes passed all 92 sections and all three rounds. The earlier failure is retained here; neither the RNG implementation nor fixture thresholds were changed. The script now preserves full failure output and reports its log path.

File-stream UTF-8 byte mark/seek is tested against the specification. CheapGlk's Unicode text streams differ by dividing marks by 4; differential agreement is not claimed for this edge case. Unicode receives `all / quit`; resource streams receive `quit`. Comparison normalizes only the interpreter version field, and Glulxe uses `-q -u` for UTF-8. Adventure's saving interpreter receives `north / save / path / quit / y`; the other receives `restore / path / look / quit / y`. Both directions restore to `In Forest` and pass.

## Test Matrix

The main local evidence is in [conformance.rs](../src/vm/conformance.rs); existing VM, Memory, Story, and GUI tests are retained.

| Checklist domain | Local regression entry points/coverage | External validation |
| --- | --- | --- |
| Decoding, addressing, stack, locals | `all_load_address_modes_and_opcode_encodings`, `narrow_copy_integer_extremes_and_stack_bounds`, zero-length memory operations, locals frame capacity | General Glulxercise |
| Calls, search, strings/filter | Original VM tests, `huffman_leaf_and_indirection_matrix`, iterative output continuations, saves inside numeric filters | General Glulxercise; 40,000 Huffman substrings match reference |
| Single/double precision | `double_*`, `floating_branches_*`, `float_nan_modulo_and_power_identities` | Glulxercise allfloat/alldouble; double stack-result reference test |
| IFZS | Round trips, corrupt saves, file prompts, empty MAll, repeated annotation/extension/singleton chunks | Synthetic-story and Adventure bidirectional interoperability |
| undo/restart/protect/heap | `undo_*`, `allocation_limits_*`, original heap regressions | General Glulxercise |
| Inform acceleration 1–13 | [acceleration.rs](../src/vm/acceleration.rs): registration/removal, classes/properties/private access, old/new layouts, call/callf/tailcall, compressed strings and 20,000 filter callbacks, restored-state boundaries | All 94 results across 13 functions exactly match Glulxe |
| random/verify/gestalt | `random_ranges_determinism_verify_and_capabilities` | General Glulxercise |
| Streams/dispatch | read/seek/Unicode/count, echo cycles, unbinding on close, same-file shared cache/independent positions/dirty flush/old-session migration, seeking at write end, UTF-8 byte marks and overwrite, old-session migration, original stack-reference tests | Exact resstreamtest reference match; shared-stream output/counts/final file bytes match differential checks |
| Windows/events | Arrangement direction/nested key/close/resize/font metrics, multi-window input, cancellation/timers, select_poll event classification, graphics clipping and background expansion | twocol startup and Sensory GUI |
| Unicode | Extended conversion, titlecase, NFC/NFD, capability arguments | Exact unicasetest reference match |
| Text images | [presentation.rs](../src/vm/presentation.rs): image order, event association, dynamic/zero sizes, sessions; [text_buffer.rs](../src/app/text_buffer.rs): inline baselines, both-side/repeated margins, flow-break, wrapping/words, resizing/clipping | Synthetic-story GUI resize/click/restore |
| Styles/mouse/links/terminators | Style snapshots and actual measurements, indentation/four justifications, echo style propagation, fixed-grid styles/links/editing, actual Chinese glyph rendering and missing-glyph capabilities | GUI validation of 25 special keys, grid LINK/prefilled editing, official timed cancellation and editing after restore |
| Date/time | Epoch, negative times, full i32 years, field/negative-microsecond normalization, UTC round trips, New York DST gaps, Apia skipped day, ancient/future offsets | datetimetest startup |
| Sound | Device-free idle sink: synchronized sample starts, independent pause/volume, stop/end/fade notifications; [tracker.rs](../src/vm/sound/tracker.rs): four-format PCM, S3M OPL, speed/BPM/volume/E6 loops, repetition/restoration offsets; sampled streaming encoding/duration/resampling boundaries | Sensory AIFF; synthetic MOD GUI playback and completion notification |
| Blorb/product | Container bounds/resource index/metadata tests, session serialization | Adventure/Sensory close-and-reopen restoration |

This matrix describes domain coverage, not exhaustive combinations of valid and invalid inputs.

## Reference Versions and Fixtures

Glulxe revision `56ab8743bab565de307bd892c555d8d8897ed517`; CheapGlk revision `14d8aaf6e4150669762bd4646a5368e75c1eeee6`, from [Glulxe](https://github.com/erkyrath/glulxe) and [CheapGlk](https://github.com/erkyrath/cheapglk). Build CheapGlk in an adjacent directory before building Glulxe. This validation used `OPTIONS='-O2 -Wall -DOS_UNIX -DUNIX_RAND_GETRANDOM'`. CheapGlk does not provide desktop graphics/sound, so those capabilities are not blindly aligned with its return values.

Except for Adventure, fixtures come from the [author's official Glulx fixture page](https://eblong.com/zarf/glulx/); download URLs append the filenames below to that directory. Adventure comes from [IF Archive](https://www.ifarchive.org/if-archive/games/glulx/advent.ulx) and is named `glulx-advent.ulx` in the fixture directory. Fixtures and reference interpreter sources are not distributed with this repository.

| File | SHA-256 |
| --- | --- |
| glulxercise.ulx | `b732127fee4cb266a5330981c1111fdfaba237134525754e063e6dc5f449b348` |
| unicasetest.ulx | `e4b2da7fe1a894913421ba87cf26551f18fa158294df2333bbb79bc39b2f219c` |
| resstreamtest.gblorb | `1d7c77d830913447e08594670c2d8ee03df75517bf46a5d2b70607422777837f` |
| glulx-advent.ulx | `264236f2c3504eb2f326ab560aef435902d357c1ef848127b6ea0329c10387f8` |
| sensory.blb | `a05cd29a71b3200e564a1f33146200f88664aab394b9924acab99381a4001afd` |
| inputfeaturetest.ulx | `1fe2d4c126dd883abfc0f19c41676d34973f424c58f00b4ab5a0ee24c348552c` |
| datetimetest.ulx | `b32ec0803c60a31de07c4c23c00bb5d4f8dbe258956392813b8af0847d71a0b5` |
| twocol.ulx | `23b2acb6ba725236b9db010342f607a41e9fc5e44758a7360b12d102418b0e6d` |

The Glulxercise binary is Release 13 / 241202; the `.inf` downloaded from the same directory is Release 10 / 220722 and must not be treated as the same version. Unicode is Release 3, resource streams Release 2, input extensions Release 1, date/time Release 4, Sensory Jam Release 4, and Adventure Release 5 / 961209.

## Desktop Validation and Limits

On Linux Xvfb with software OpenGL, focus the window through X11, send input, exit normally using `WM_DELETE_WINDOW`, then restart without a story argument:

- Adventure: `north`, close, reopen; the persistent session retains `In Forest`.
- Sensory Jam: `hit gong / east / examine photograph`; AIFF playback reports success without an unsupported message, and the image is visible; text and graphics canvases survive close/reopen.

Sound validation confirms device/decoder/playback paths and event state; it does not include human listening or sampled-waveform comparison. Input-extension, date, and multi-window fixtures also pass headless startup, command, and exit smoke tests; this does not mean all their interactions were automatically verified. Windows/macOS GUI tests have not been run in this round. No complete long-game walkthrough has been finished, and not all media encodings or physical sound-card waveforms are claimed verified. Sound2 multi-channel synchronization has been checked by stereo sample frame at the software output layer.

Real terminal validation requires PTY/TTY checks for grid display, prefilled editing, timed cancellation, and unechoed single-key input. Existing pipe transcript comparisons are not passing evidence for that capability.

Further validation must record Windows/macOS system and build versions, individual operation results, and failures. Full game runs require pinned game versions and reproducible routes; media variants require encoding parameters, output, and failure behavior. New results should state platform and coverage before corresponding tasks are checked off.

## Added Synthetic Media Validation

This fixture is generated entirely by repository scripts, with original PNG and four-channel MOD content and no downloaded game dependency:

```sh
python3 tools/make-media-fixture.py /tmp/glulx-media.gblorb
cargo run -- /tmp/glulx-media.gblorb
```

Under Linux Xvfb, resize the window to 1100×820 and 700×820: all three inline image alignments are correct, left/right margins shrink with window width, and text wraps beside images then returns to full width below them. Clicking an image produces `Image hyperlink received.`; audio completion produces `MOD playback completed.`. After normal exit and restart without a story argument, the text, images, and pending input remain. The original Sensory Jam AIFF/photo/restoration checks were rerun to confirm media changes preserved existing paths.

The synthetic story ignores non-input events such as Arrange so resizing cannot incorrectly end the test. Rust tests cover zero-size images, invalid margin placement, ineffective flow-breaks, oversized images, and font wrapping. Fade regressions cover replacement midway and completed-fade notifications when polling is delayed, ensuring continuation from the current volume.

## Style, Input, and Graphics Edge-Case Validation

The original style fixture covers centered headings, hanging indents and justified paragraphs, right alignment, grid LINK, and prefilled input:

```sh
python3 tools/make-style-fixture.py /tmp/glulx-styles.ulx
cargo run -- /tmp/glulx-styles.ulx
python3 tools/check-input-ui.py --candidate target/debug/glulx-rs --output /tmp/glulx-input-ui --input-feature /path/to/inputfeaturetest.ulx
```

The style story passed under Linux Xvfb: click grid LINK, change prefilled Ada to Grace Hopper, submit, and close/reopen normally with events, text, and grid retained. Rust rendering tests also verify actual Chinese glyphs, compression of wide glyphs into a single cell, oblique/weight/color, grid link hits, and editing.

The input script allocates an X display automatically and retains screenshots, application logs, and persistent sessions; it exits through normal WM_DELETE_WINDOW. The original story checks exact values for 25 native key events (F1–F12, arrows, Delete/Backspace, Esc/Tab/Page/Home/End/Enter), the composed abc at timed cancellation, and NEW prefilled content when re-requested in the same VM slice. The official Input Feature Test checks ROT13 display after timed cancellation, retention of the original abcdef, and continued editing after session restoration.

Graphics regressions verify that resizing immediately retains visible top-left pixels, clips removed areas, and fills new areas with the current background. Growing after shrinking does not restore clipped pixels; zero size releases the canvas. Unsigned rectangle dimensions clip as specified, including 0xFFFFFFFF combined with negative coordinates.

## Media Formats and Further Edge Cases

```sh
python3 tools/check-graphics-ui.py --candidate target/debug/glulx-rs --output /tmp/glulx-graphics-ui
python3 tools/check-audio-codecs.py
```

The graphics script passed: shrinking clips the right-hand green square, which does not reappear after growing; the top-left red square remains and new areas use the current background. Drawing an image with width/height 0xFFFFFFFF and a negative origin succeeds and persists through normal close/restore. The script checks screenshot pixels and persistent sessions directly, beyond process survival. Rust regressions also check failure on corrupt IDAT, alpha compositing, and adaptation of valid 20000×1 images to GPU texture-dimension limits before upload to avoid debug panics; the VM retains original image dimensions.

Blorb RIdx must be first and unique. RDes parsing validates UTF-8, the absence of inter-entry padding, truncation, and duplicate entries; image/sound descriptions appear in Story information. Reference: [Blorb 2.0.5](https://eblong.com/zarf/blorb/Blorb-Spec.md). Duplicate IFZS chunks follow [Quetzal 1.4 §8.8–8.9](https://www.ifarchive.org/if-archive/infocom/interpreters/specification/savefile_14.txt).

In raw-bytecode date differential tests, 10^13 seconds returns YEAR:318857, matching Glulxe. Raw-bytecode shared-file checks verify cross-handle reads of newly written content, independent read/write counts, and final file XYC; these are integrated into check-reference.py.

The four original generated tracker fixtures are in [fixtures.rs](../src/vm/sound/tracker/fixtures.rs). MOD/XM/S3M/IT verify non-silence, completion, complete repetitions, and frame-aligned restoration; S3M additionally checks OPL instrument sound and volume attenuation. [sampled.rs](../src/vm/sound/sampled.rs) decodes sampled sources packet by packet, sharing encoded data and trimming encoder padding using integer container frame counts. Repetitions count natural EOFs, independently of floating-point Duration.

The audio codec tool generates original 44.1 kHz stereo tones with ffmpeg, then tests the project's currently built decoder: AIFF/OGG/MP3 at 250 ms (22,050 samples) and 1250 ms (110,250 samples). All six groups pass exact sample counts, duration, finite/infinite repetition, 5 ms restoration, and playback conversion checks. Cargo JSON artifact output identifies the Rodio/Symphonia build, avoiding stale feature combinations. The tool requires cargo/rustc/ffmpeg; the runtime player does not depend on ffmpeg.

A further 18 audio checks verify that 8/22.05/48 kHz input resampled to 44.1 kHz matches a fully buffered baseline sample for sample; decoder packet and repetition boundaries do not reset resampling phase.


## Separate Resources, Terminal Input, SONG and Light Weight

```sh
python3 tools/check-resource-maps.py --fixture /path/to/resstreamtest.gblorb --candidate target/debug/glulx-rs --reference /path/to/glulxe --output /tmp/resource-maps
python3 tools/check-resource-ui.py --candidate target/debug/glulx-rs --output /tmp/resource-ui
python3 tools/check-terminal.py --candidate target/debug/glulx-rs
python3 tools/make-song-fixture.py /tmp/glulx-song.gblorb
python3 tools/check-song-ui.py --candidate target/debug/glulx-rs --output /tmp/song-ui
```

- Resource model: eight new Rust tests cover resource-only archives, 128-byte IFhd validation, conflicts, atomic failure, discovery precedence/ambiguity, loose file types and old/current desktop snapshots. The official resource-stream story produces identical output as a bundled story, raw story plus explicit archive, auto-discovered archive and loose directory; output also matches Glulxe after interpreter-version normalization.
- Resource GUI: CLI selection and all three GUI choices, archive Browse and Use this directory, changing picture caches, bad IFhd retaining existing state, resource-path focus, and opening another story without resource leakage passed. After original archives/directories and the raw story are deleted, restored sessions read Data and redraw pictures again, proving resource content was restored rather than only a cached canvas.
- TTY: actual PTY checks passed 25 immediate special-key events, prefill editing, timed cancellation/current composition, replacement prefill, grid editing, multi-window selection, resize/Arrange, echo/terminators, file writing/cancellation, and terminal restoration after normal exit, Ctrl+C/Ctrl+D and VM errors. Pipe output remains exact. Windows console attachment code is present but has not received Windows on-device validation.
- SONG: nine focused tests cover 15/31-sample headers, shared/22-byte references, AIFF 1–32-bit PCM, SSND offsets, MARK/INST no/forward/ping-pong loops, malformed references and bounds, equivalent-module PCM, repeats, offsets, pause/stop and notifications. The original desktop fixture passes active paused-session restoration, resume/fade completion, the last finite-repeat event, two simultaneous channels and stop.
- Light weight: metadata checks reject falsely named regular/invalid fonts; actual installed light glyphs render in named font families, and host availability is reinstalled after serialization. Linux desktop output reports both buffer and grid weights as -1, with links, inline editing and session restoration still working. Missing faces retain regular fallback.

The reference regression still passes all ten checks, including 92 Glulxercise sections and both Adventure save directions. Resource, terminal and SONG tools generate their own temporary content and do not download games.
