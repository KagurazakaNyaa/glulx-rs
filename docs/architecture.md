# Architecture

[English](architecture.md) | [中文](architecture.ZH.md)

The interpreter is Rust, with no runtime dependency on a C interpreter. Eframe/egui
provides the Windows, Linux and macOS GUI; the same binary also has a line-oriented headless automation adapter. A full
terminal display/input host is selected for interactive TTYs.

## Modules

`Story` validates the executable header and memory layout, parses Blorb resource
indexes, and exposes metadata and cover resources. Its executable image is shared
with `Memory`'s initial-image baseline; file loading transfers owned container bytes
where possible. `Memory` enforces ROM protection, address checks and a bounded allocation policy.

`Vm` owns instruction decoding, execution, stacks and the Glk object model. Its
bounded run interface keeps the GUI responsive. Unsupported instructions are typed
errors carrying execution context. Host behavior is split into modules under
`src/vm`: `windows`, `streams`, `events`, `presentation`, `unicode`, `datetime`,
`sound`, `save`, `session`, `acceleration`, `strings` and `grid`. The VM depends on neither egui nor HTTP; sound uses
rodio; MOD/XM/S3M/IT resources are generated incrementally by a Rust tracker player;
sampled formats repeat at decoder EOF without expanding the whole clip in memory. Multi-play
channels enter the device as one aligned source. The picture module validates source images, shares the first decode with graphics
draw requests, and samples only visible destination
pixels, so oversized draw requests do not allocate oversized images. Presentation returns window rectangles,
text runs with image/flow markers, and graphics commands. The GUI text-buffer layout
formats inline images and floating margins together with styled text, retaining image
rules for resize and caching textures per story. Styles resolve once through a shared
VM/presentation model so style_measure matches rendered sizes/colors. Grid cells retain
style and hyperlink attributes with consistent paint and input geometry. Host font
coverage and metrics are injected callbacks, omitted from portable and desktop
serialization and reinstalled by the GUI. This keeps egui out of the VM.

The VM has one owner on the UI event-loop thread; this egui host uses 8 ms quanta (checked every 1024 instructions) to remain responsive. It blocks only at input/event waits; UI input is delivered directly to the same VM state. Empty select_poll calls without intervening host operations no longer force a frame. Translation requests and audio playback use background work; GPU uploads remain on the UI thread.
`PlayerApp` supplies keyboard,
mouse, hyperlink and file-selection results. Native companion viewports contain log/input, translation and settings; only the root
canvas determines Glk dimensions. Graphics retain clipped image/rectangle primitives, published with shared window
views at event boundaries. Hardware OpenGL scales and blends them on the GPU.
Software drivers use an incremental CPU bitmap. Opaque draws remove covered
commands; long histories compact, and the recent-image cache has a configurable pixel-payload budget (default 512 MiB).
CPU rasterization also preserves the existing desktop snapshot format.
Waiting states distinguish line,
character, file (including overwrite confirmation) and general events. The pipe adapter reads stdin on a worker
so waiting for a line does not prevent timer delivery. Streams share file contents across handles, retain independent cursors and flush only
dirty content at stop/save.

`Story` keeps the executable container separate from an optional external resource
map. Archive/directory selection validates identity before replacing resource state;
loose directories become self-contained resource Blorbs for desktop serialization.
The GUI applies selections through a fresh VM. `terminal` chooses a full TTY screen
and editor or the pipe protocol using stdin/stdout terminal detection. The SONG
assembler resolves AIFF references before handing a module to the existing tracker.

## Persistence

Player settings use an executable-adjacent `glulx-settings.json`, migrated from
the old eframe settings key when absent. Writes use a same-directory temporary
file and rename. Invalid JSON is reported and preserved instead of overwritten.
Desktop session storage remains separate.

`save` implements portable IFZS. It validates identity, memory, heap and stack
continuations before replacing execution state. RNG, Glk, I/O system, the string
table and protection definition remain independent of restore/undo/restart.
Undo stores bounded execution snapshots, sharing the same state boundary. Its payload
budget counts retained dirty pages, stack bytes and heap-record data, not the shared
story image or current VM address space.
Acceleration registrations/parameters are also outside portable snapshots. Accelerated
calls use deferred completion so nested filter/string continuations do not recurse
on the native stack; desktop snapshots retain pending completion.

`session` validates a versioned desktop snapshot that also retains story resources,
Glk objects, pending input and audio progress. The app adds graphics canvases and
resumes timers/audio. Eframe storage saves every 30 seconds and on normal exit;
startup without an explicit story attempts to resume it. The log transcript retains
at most 4 MiB or 100,000 lines and uses virtualized display rows. This local snapshot
is separate from game-requested, portable saves. The synchronous desktop serializer
expands byte arrays into RON integer lists. A 16 MiB raw-payload guard skips
large story/memory/canvas snapshots before serialization and clears any stale
previous session; settings and game-requested IFZS saves remain available.

## Translation

`Translator` copies narrative text to an asynchronous worker at input waits.
Translation cannot mutate execution, input or save state. Request IDs retain turn
order; source text and request configuration form the cache key. Identical
in-flight requests share a worker call and fan out to distinct turn IDs. Only
successful results are cached. Toggle transitions clear unfinished capture,
without altering recorded turn state or backfilling old content. An opt-in VM
text-buffer event stream carries narrative text and clear boundaries, excludes
input echo, and is omitted from snapshots. Clears discard pending text from that
window and start a new view; same-view batches append, old views are collapsed
in History, and exact redraws reuse the current view. Original output is always immediately
available. Credentials are local settings, not release artifacts.

Glulxe is used for output and save interoperability checks; Git and Gargoyle
remain behavioral and portability references. No C interpreter source is compiled
into the player.

Text-buffer layout caches are invalidated by content/style revisions, width, DPI, font selection and hyperlink colors. Unchanged window views retain their presentation snapshot across timer events. Idle repaint requests use the next Glk timer deadline, capped by the 100 ms host idle interval.
