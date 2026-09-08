# Compatibility

For the dated source audit, public specification links, and actionable tasks,
see the [Glulx specification checklist](glulx-spec-checklist.md). It distinguishes
implementation coverage from full conformance testing and includes the implementation progress at its recorded commit and audit time.

## Implemented

- Raw Glulx executables and Blorb containers containing a `GLUL` chunk.
- Glulx 2.x and 3.0/3.1 header validation, checksum validation, and memory map.
- ROM write protection, RAM reset, resize, byte/short/word access, and overlap-safe copy.
- Variable-length opcodes and all standard load/store address modes.
- C0/C1 function frames, calls, tail calls, returns, locals, and value stack operations.
- Integer arithmetic, bit operations, branches, array loads/stores, all three search opcodes, random values, and basic floating-point operations.
- Null, filter, and Glk output systems; byte, Unicode, and Huffman-compressed strings, including embedded function calls.
- Minimal Glk window, stream, line input, character input, select, and Unicode output calls; style calls currently include no-op placeholders.
- Single-level `saveundo`/`restoreundo`, plus `hasundo`/`discardundo`.
- Heap allocation/deallocation and heap state handling across undo/restart (commit `31bb75c`).
- Memory-stream output, separate text-grid output, Blorb picture resources, and basic graphics-window rendering.
- GUI open/restart/stop, drag-and-drop, scrollback, appearance settings, status display, and asynchronous translation.

## Missing Or Incomplete

- Quetzal save/restore and autosave.
- Acceleration setup opcodes and Inform acceleration functions.
- Double-precision floating-point opcodes.
- Full Glk object dispatch, multiple window layout, hyperlinks, timers, mouse events, file references, and streams.
- Sound/music, cover art, and iFiction metadata.
- Full text-grid and graphics-window layout/behavior conformance; basic rendering already exists.

Stories that execute unsupported VM opcodes stop with an explicit compatibility
error. Unsupported or partial Glk calls can instead return zero or omit behavior,
so successful startup does not establish full playability. The next useful
milestone is persistent save/restore with Glk file support and completion of the
Glk object/event model; optional capabilities should follow target-story needs.

The public `advent.ulx` from IF Archive is used as a smoke test. The current VM
can start it, render the initial room, process several text commands, and quit
cleanly. This is a narrow compatibility signal, not a claim that the whole game
or modern Glulx stories are fully supported.

## Compatibility Policy

The intended policy is to advertise only implemented gestalt capabilities;
the checklist records remaining declaration and argument-handling audits. Unknown Glk
selectors return zero and are recorded internally so probing code can continue.
Unsupported VM opcodes and string types are fatal typed errors because silently
continuing would corrupt execution state.
