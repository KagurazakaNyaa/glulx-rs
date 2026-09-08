# Compatibility

The implemented target is Glulx 3.1.3 with Glk 0.7.6. See the
[specification checklist](glulx-spec-checklist.md) and [validation record](glulx-validation.md)
for coverage and reproducible evidence. This is not a claim of exhaustive conformance.

## Implemented

- Validated raw Glulx and indexed Blorb executables, images, sound/data resources, iFiction metadata and cover art.
- Standard integer, addressing, call, search, string, single/double precision and heap instructions.
- IFZS persistent saves interoperating with Glulxe, restart/protect and multiple undo states.
- Inform acceleration functions 1–13, including class/property access and legacy/current object layouts; unsupported registrations are removed as specified.
- Pair window trees, text buffers/grids, graphics windows, file/memory/resource streams and Unicode operations.
- Multiple input requests, event queues, timers, mouse, hyperlinks, line terminators and echo control.
- Text styles, inline/margin images, date/time APIs and audio channels with repeat, pause, fades and notifications.
- Desktop file prompts, open/restart/stop, scrollback, settings, translation and automatic session restoration.

## Limits

VM memory is limited to 256 MiB. Undo retains at most 16 snapshots within a
64 MiB combined budget; older snapshots are evicted and an oversized save fails.
Portable IFZS excludes Glk, RNG, I/O system and string-table state as required;
the separate versioned desktop session includes host state and bundled story data.
Desktop sessions are local player snapshots, not an interchange format.

Text buffers support all five image alignments, margin flow breaks, image hyperlinks
and Glk 0.7.6 dynamic width/aspect/maximum-width rules. Graphics-window images retain
their dimensions at draw time. Text-buffer style hints 3–9 are supported; paragraph
hints 0–2 and grid style hints are ignored. Fonts and glyph availability depend
on egui and installed/configured fonts.

Sound decoding uses rodio/Symphonia plus pure Rust xmrs/xmrsplayer for
ProTracker/SoundTracker MOD music. MOD is streamed to PCM rather than fully expanded
before playback. XM/S3M/IT and bit-exact reproduction of every historic MOD variant
are not claimed. `play_multi` submits a single combined output source, aligning
channels to the same stereo sample frame; physical sound-card output is not measured.
AIFF and generated MOD resources have GUI coverage, while not every codec/encoding
combination has a fixture.

The headless adapter exposes terminal text and file prompts and does not advertise
GUI-only facilities. Without an audio output device the desktop disables sound
capabilities. Gestalt queries account for their arguments and host availability.
Unknown Glk selectors return zero and are recorded; unsupported VM instructions
produce typed errors. A successfully started game is not necessarily fully playable.

Validation includes official Glulxercise, Unicode and resource-stream suites,
Adventure save interoperability and Linux desktop smoke tests. Windows/macOS
runtime behavior and complete game walkthroughs remain outside this validation.
