# Compatibility

[English](compatibility.md) | [中文](compatibility.ZH.md)

The implemented target is Glulx 3.1.3 with Glk 0.7.6. See the
[validation record](glulx-validation.md) for reproducible evidence and the
[current specification audit](glulx-remaining-spec-audit.md) for open boundaries.
This is not a claim of exhaustive conformance.

## Implemented

- Validated raw Glulx and indexed Blorb executables, images, sound/data resources, iFiction metadata, cover art and RDes image/sound descriptions.
- Standard integer, addressing, call, search, string, single/double precision and heap instructions.
- IFZS persistent saves interoperating with Glulxe, restart/protect and multiple undo states.
- Inform acceleration functions 1–13, including class/property access and legacy/current object layouts; unsupported registrations are removed as specified.
- Pair window trees, text buffers/grids, graphics windows, file/memory/resource streams and Unicode operations.
- Multiple input requests, event queues, timers, mouse, hyperlinks, line terminators and echo control.
- Text styles, inline/margin images, date/time APIs and audio channels with repeat, pause, fades and notifications.
- Desktop file prompts, open/restart/stop, scrollback, settings, translation and automatic session restoration.

## Resource Selection and Terminal Playback

Raw `.ulx` stories accept separate resource-only Blorbs or loose resource directories
through `Story::open_with_resources`, `--resources PATH`, and File → Choose resources.
An archive with `IFhd` must match the first 128 bytes of the Glulx story; absence is
allowed. Conflicting executables, corrupt archives and wrong identities fail before
replacing state. The GUI applies a new selection by restarting the story, clearing
image/cover caches and active audio; a failed selection preserves the current game.

Automatic discovery beside raw stories uses `.blorb`, `.blb`, `.gblorb`, then `.glb`.
The stem must match exactly; extensions ignore ASCII case, and multiple candidates
at the same priority are an error. `--resources` overrides discovery; `--no-auto-resources`
disables it. Resources fully replace the selected resource map. Desktop sessions
embed resource content and retain the origin, so restoration works after the original
archive/directory is removed. Portable IFZS retains its existing VM-only scope.

Loose directories are explicit and non-recursive. Numbered `PIC` files support PNG/JPEG,
`SND` supports AIFF/OGG/MP3/MOD/XM/S3M/IT/SONG, and `DATA` supports TEXT/BINA/FORM.
Names and numeric prefixes ignore ASCII case; duplicate resource numbers and invalid
numbered resources fail. Metadata names include IDENT, FRONTIS, RESDESC and METADATA.
Unrelated files are ignored; a STORY resource is rejected as a conflicting executable.
See the resource-map validation tool for exact suffixes and examples.

With `--headless`, an interactive TTY displays text buffers, grids and status windows,
with prefilled editing, timed cancellation and immediate unechoed character input.
Ctrl+N selects the next pending input window; Ctrl+C exits, and Ctrl+D exits from an
empty editor. Terminal modes and the original screen are restored on normal/error exit.
When either stdin or stdout is piped, a stable text/file protocol is used instead;
unsupported grid and graphics windows fail creation. The terminal advertises no
image, mouse, hyperlink-input or sound support. Characters that do not occupy one
terminal column display as `?` and report CannotPrint; Unicode input/file data is retained.

## Limits

VM memory defaults to 1024 MiB and is configurable. Undo retains at most 16 snapshots
within a configurable payload budget (default 256 MiB), counting retained page snapshots,
stack bytes and heap-record payloads; the shared story image and current VM address space are
not charged, and allocator/container overhead is excluded. The log transcript retains
at most 4 MiB or 100,000 lines and uses virtualized display rows; this limits player history,
not the VM's raw output. See the
[separate resource and process limits](../README.md#performance-and-memory-limits). Older snapshots are evicted and an oversized save
fails. setmemsize/malloc handle their configured limits and memory reservation
failures; this is not a general promise of recovering from every process allocation failure.
Portable IFZS excludes Glk, RNG, I/O system and string-table state as required;
the separate versioned desktop session includes host state and bundled story data.
Desktop sessions are local player snapshots, not an interchange format.

Text buffers support all five image alignments, margin flow breaks, image hyperlinks
and Glk 0.7.6 dynamic width/aspect/maximum-width rules. Graphics-window images retain
their dimensions at draw time; scaling samples only visible canvas pixels, including
very large unsigned target sizes. PNG/JPEG resources are fully decoded before a
successful Glk image result. Decoded sources default to a configurable 256 MiB RGBA output limit, with a
decoder allocation budget twice the output limit; unavailable images report failure. RDes text
alternatives are available in Story information. Text-buffer style hints 0–9 are supported; paragraph
indentation/hanging indents and all four justification modes are implemented. Grid
cells retain style and hyperlinks; grid layout hints 0–3/6 are ignored to keep equal
cell dimensions, while weight/oblique/color hints apply. Measurements reflect actual
rendering; light requests select actual light faces for proportional/monospace fonts
when available, and otherwise report regular fallback. Font metadata is checked rather
than trusting filenames; host face availability is reinstalled after session restoration. System outline
fonts are loaded automatically, and Options accepts an extra TTF/OTF/TTC fallback.
CharOutput reports missing glyphs accurately. Text-buffer window sizing uses the
normal style’s actual font metrics; grids keep uniform 8×16 cells.

Sound decoding uses rodio/Symphonia plus pure Rust xmrs/xmrsplayer for
all four standard Blorb tracker formats: MOD, XM, S3M and IT. Tracker and sampled
audio are decoded as played. Finite repetitions end at natural decoder EOF, retaining
every PCM sample and stereo frame. Container frame counts remove encoder padding,
and resampling remains continuous across packet and repetition boundaries. Bit-exact reproduction of every historic tracker
variant is not claimed. `play_multi` submits a single combined output source, aligning
channels to the same stereo sample frame; physical sound-card output is not measured.
AIFF, generated MOD and SONG resources have GUI coverage, while not every
codec/encoding combination has a fixture. The optional legacy SONG format resolves
shared `SND<number>` AIFF samples, including SSND offsets and MARK/INST sustain loops
(no loop, forward and ping-pong). Samples become 16-bit mono (channels averaged,
low bits discarded for higher bit depths); pitch follows MOD period/finetune.
SONG uses the configurable encoded audio-resource limit (default 256 MiB) and PCM budget (default 128 MiB).
Missing/invalid references fail; repeat, pause, notification and restore use the common audio path.

File streams preserve encoded-byte positions and Unicode overwrite semantics, with
shared contents and independent positions for concurrent streams of the same file. File
prompts reject missing read paths and confirm modifications to existing files.
Input cancellation echoes the retained composition, line terminators are bound to
each request, and select_poll leaves player input for select. The GUI supports
standard special keys and editing directly in grid windows.

The headless adapter exposes terminal text and file prompts and does not advertise
GUI-only facilities. Without an audio output device the desktop disables sound
capabilities. Gestalt queries account for their arguments and host availability.
Unknown Glk selectors return zero and are recorded by default; `--strict-glk` promotes
them to a typed error for reference checks, and `--trace-events` writes delivered story
events as JSON. Unsupported VM instructions produce typed errors. A successfully started
game is not necessarily fully playable.

Date conversion uses Gregorian arithmetic across the signed 32-bit year range;
local time retains historical offsets and future recurring timezone rules. IFZS
restoration accepts repeatable annotation/extension chunks and ignores later duplicate
singleton chunks as specified by Quetzal.

Validation includes official Glulxercise, Unicode and resource-stream suites,
Adventure save interoperability and Linux desktop smoke tests. Windows/macOS
runtime behavior and complete game walkthroughs remain outside this validation.
