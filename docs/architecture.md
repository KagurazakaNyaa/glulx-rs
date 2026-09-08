# Architecture

[English](architecture.md) | [中文](architecture.ZH.md)

The interpreter is Rust, with no runtime dependency on a C interpreter. Eframe/egui
provides the Windows, Linux and macOS GUI; the same binary also has a line-oriented headless automation adapter. A full
terminal display/input host is tracked in the specification checklist.

## Modules

`Story` validates the executable header and memory layout, parses Blorb resource
indexes, and exposes metadata and cover resources. `Memory` enforces ROM protection,
address checks and a bounded allocation policy.

`Vm` owns instruction decoding, execution, stacks and the Glk object model. Its
bounded run interface keeps the GUI responsive. Unsupported instructions are typed
errors carrying execution context. Host behavior is split into modules under
`src/vm`: `windows`, `streams`, `events`, `presentation`, `unicode`, `datetime`,
`sound`, `save`, `session`, `acceleration`, `strings` and `grid`. The VM depends on neither egui nor HTTP; sound uses
rodio; MOD/XM/S3M/IT resources are generated incrementally by a Rust tracker player;
sampled formats repeat at decoder EOF without expanding the whole clip in memory. Multi-play
channels enter the device as one aligned source. The picture module validates source images and samples only visible destination
pixels, so oversized draw requests do not allocate oversized images. Presentation returns window rectangles,
text runs with image/flow markers, and graphics commands. The GUI text-buffer layout
formats inline images and floating margins together with styled text, retaining image
rules for resize and caching textures per story. Styles resolve once through a shared
VM/presentation model so style_measure matches rendered sizes/colors. Grid cells retain
style and hyperlink attributes with consistent paint and input geometry. Host font
coverage and metrics are injected callbacks, omitted from portable and desktop
serialization and reinstalled by the GUI. This keeps egui out of the VM.

`PlayerApp` executes short slices, renders each window, and supplies keyboard,
mouse, hyperlink and file-selection results. Waiting states distinguish line,
character, file (including overwrite confirmation) and general events. The terminal adapter reads stdin on a worker
so waiting for a line does not prevent timer delivery. Streams share file contents across handles, retain independent cursors and flush only
dirty content at stop/save.

## Persistence

`save` implements portable IFZS. It validates identity, memory, heap and stack
continuations before replacing execution state. RNG, Glk, I/O system, the string
table and protection definition remain independent of restore/undo/restart.
Undo stores bounded execution snapshots, sharing the same state boundary.
Acceleration registrations/parameters are also outside portable snapshots. Accelerated
calls use deferred completion so nested filter/string continuations do not recurse
on the native stack; desktop snapshots retain pending completion.

`session` validates a versioned desktop snapshot that also retains story resources,
Glk objects, pending input and audio progress. The app adds graphics canvases and
resumes timers/audio. Eframe storage saves every 30 seconds and on normal exit;
startup without an explicit story attempts to resume it. This local snapshot is
separate from game-requested, portable saves.

## Translation

`Translator` copies narrative text to an asynchronous worker at input waits.
Translation cannot mutate execution, input or save state. Request IDs retain turn
order; the source text is the cache key. Original output is always immediately
available. Credentials are local settings, not release artifacts.

Glulxe is used for output and save interoperability checks; Git and Gargoyle
remain behavioral and portability references. No C interpreter source is compiled
into the player.
