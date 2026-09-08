# Compatibility

[English](compatibility.md) | [中文](compatibility.ZH.md)

The implemented target is Glulx 3.1.3 with Glk 0.7.6. See the
[specification checklist](glulx-spec-checklist.md) and [validation record](glulx-validation.md)
for coverage and reproducible evidence. This is not a claim of exhaustive conformance.

## Implemented

- Validated raw Glulx and indexed Blorb executables, images, sound/data resources, iFiction metadata, cover art and RDes image/sound descriptions.
- Standard integer, addressing, call, search, string, single/double precision and heap instructions.
- IFZS persistent saves interoperating with Glulxe, restart/protect and multiple undo states.
- Inform acceleration functions 1–13, including class/property access and legacy/current object layouts; unsupported registrations are removed as specified.
- Pair window trees, text buffers/grids, graphics windows, file/memory/resource streams and Unicode operations.
- Multiple input requests, event queues, timers, mouse, hyperlinks, line terminators and echo control.
- Text styles, inline/margin images, date/time APIs and audio channels with repeat, pause, fades and notifications.
- Desktop file prompts, open/restart/stop, scrollback, settings, translation and automatic session restoration.

## Not yet implemented

Raw `.ulx` files cannot attach a separate Blorb resource archive through the GUI,
CLI or public API, and the player does not discover same-name archives. Resources
are currently read only from the story's own Blorb container. Separate archive
selection and session restoration are tracked in the specification checklist. Blorb
IFhd identity checks and conflicting executable/resource arguments also need
coverage. Loose PIC/SND/DATA resource-directory loading is an optional missing
player feature. Container validation currently tolerates nonzero padding and an
unindexed GLUL fallback; it does not reject every malformed archive.

The deprecated, optional Blorb `SONG` format, which references AIFF samples stored
as separate resources, is not implemented.

The current headless loop is a line-oriented automation adapter. It does not render
text-grid/status windows or provide terminal editing of prefilled text, live input
cancellation, or immediate character input without Enter. A real terminal host is
pending; its capability reporting must match its supported windows and input.

Light font-weight requests currently
render as regular weight; selecting a real light face is also pending.

## Limits

VM memory is limited to 256 MiB. Undo retains at most 16 snapshots within a
64 MiB estimated budget counting memory, stack and story image; heap-index and
allocator overhead are excluded. Older snapshots are evicted and an oversized save
fails. setmemsize/malloc handle their configured limits and memory reservation
failures; this is not a general promise of recovering from every process allocation failure.
Portable IFZS excludes Glk, RNG, I/O system and string-table state as required;
the separate versioned desktop session includes host state and bundled story data.
Desktop sessions are local player snapshots, not an interchange format.

Text buffers support all five image alignments, margin flow breaks, image hyperlinks
and Glk 0.7.6 dynamic width/aspect/maximum-width rules. Graphics-window images retain
their dimensions at draw time; scaling samples only visible canvas pixels, including
very large unsigned target sizes. PNG/JPEG resources are fully decoded before a
successful Glk image result. Decoded sources are limited to 16 megapixels with a
128 MiB decoder allocation budget; unavailable images report failure. RDes text
alternatives are available in Story information. Text-buffer style hints 0–9 are supported; paragraph
indentation/hanging indents and all four justification modes are implemented. Grid
cells retain style and hyperlinks; grid layout hints 0–3/6 are ignored to keep equal
cell dimensions, while weight/oblique/color hints apply. Measurements reflect actual
rendering; light-weight requests currently fall back to regular. System outline
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
AIFF and generated MOD resources have GUI coverage, while not every codec/encoding
combination has a fixture.

File streams preserve encoded-byte positions and Unicode overwrite semantics, with
shared contents and independent positions for concurrent streams of the same file. File
prompts reject missing read paths and confirm modifications to existing files.
Input cancellation echoes the retained composition, line terminators are bound to
each request, and select_poll leaves player input for select. The GUI supports
standard special keys and editing directly in grid windows.

The headless adapter exposes terminal text and file prompts and does not advertise
GUI-only facilities. Without an audio output device the desktop disables sound
capabilities. Gestalt queries account for their arguments and host availability.
Unknown Glk selectors return zero and are recorded; unsupported VM instructions
produce typed errors. A successfully started game is not necessarily fully playable.

Date conversion uses Gregorian arithmetic across the signed 32-bit year range;
local time retains historical offsets and future recurring timezone rules. IFZS
restoration accepts repeatable annotation/extension chunks and ignores later duplicate
singleton chunks as specified by Quetzal.

Validation includes official Glulxercise, Unicode and resource-stream suites,
Adventure save interoperability and Linux desktop smoke tests. Windows/macOS
runtime behavior and complete game walkthroughs remain outside this validation.
