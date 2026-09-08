# Architecture

The interpreter is Rust, with no runtime dependency on a C interpreter. Eframe/egui
provides the Windows, Linux and macOS GUI; the same binary also has a headless adapter.

## Modules

`Story` validates the executable header and memory layout, parses Blorb resource
indexes, and exposes metadata and cover resources. `Memory` enforces ROM protection,
address checks and a bounded allocation policy.

`Vm` owns instruction decoding, execution, stacks and the Glk object model. Its
bounded run interface keeps the GUI responsive. Unsupported instructions are typed
errors carrying execution context. Host behavior is split into modules under
`src/vm`: `windows`, `streams`, `events`, `presentation`, `unicode`, `datetime`,
`sound`, `save` and `session`. The VM depends on neither egui nor HTTP; sound uses
rodio, while presentation returns window rectangles, text runs and graphics commands.

`PlayerApp` executes short slices, renders each window, and supplies keyboard,
mouse, hyperlink and file-selection results. Waiting states distinguish line,
character, file and general events. The terminal adapter reads stdin on a worker
so waiting for a line does not prevent timer delivery. Streams flush at stop/save.

## Persistence

`save` implements portable IFZS. It validates identity, memory, heap and stack
continuations before replacing execution state. RNG, Glk, I/O system, the string
table and protection definition remain independent of restore/undo/restart.
Undo stores bounded execution snapshots, sharing the same state boundary.

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
