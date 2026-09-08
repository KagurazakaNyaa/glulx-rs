# Compatibility

## Implemented

- Raw Glulx executables and Blorb containers containing a `GLUL` chunk.
- Glulx 2.x and 3.0/3.1 header validation, checksum validation, and memory map.
- ROM write protection, RAM reset, resize, byte/short/word access, and overlap-safe copy.
- Variable-length opcodes and all standard load/store address modes.
- C0/C1 function frames, calls, tail calls, returns, locals, and value stack operations.
- Integer arithmetic, bit operations, branches, array loads/stores, all three search opcodes, random values, and basic floating-point operations.
- Null and Glk output systems; byte, Unicode, and Huffman-compressed strings, including embedded function calls.
- Minimal Glk window, stream, style, line input, character input, select, and Unicode output calls.
- Graceful failure results for unavailable `saveundo` and `restoreundo`, allowing stories that
  treat undo as optional to continue.
- GUI open/restart/stop, drag-and-drop, scrollback, appearance settings, status display, and asynchronous translation.

## Not Yet Implemented

- Catch/throw continuations.
- Quetzal save/restore, undo, protected-memory restart, and autosave.
- Heap allocation and Inform acceleration functions.
- Double-precision floating-point opcodes.
- Full Glk object dispatch, multiple window layout, hyperlinks, timers, mouse events, file references, and streams.
- Blorb images, sound/music, cover art, and iFiction metadata.
- Text grid rendering and graphics windows.

Modern Inform stories normally use several items in the second list. They will
currently stop with an explicit compatibility error rather than play to
completion. The next useful milestone is the remaining core opcodes and a real
Glk object model, followed by Blorb media resources.

The public `advent.ulx` from IF Archive is used as a smoke test. The current VM
can start it, render the initial room, process several text commands, and quit
cleanly. This is a narrow compatibility signal, not a claim that the whole game
or modern Glulx stories are fully supported.

## Compatibility Policy

The VM advertises only implemented Glulx gestalt capabilities. Unknown Glk
selectors return zero and are recorded internally so probing code can continue.
Unsupported VM opcodes and string types are fatal typed errors because silently
continuing would corrupt execution state.
