# Architecture

## Goals

- Keep the VM implementation in Rust with no dependency on a C interpreter.
- Match the familiar Windows Git player workflow.
- Use one GUI implementation on Windows, Linux, and macOS.
- Produce a single Windows executable.
- Keep translation outside VM state and game semantics.

## Module Shape

```text
story file
   |
   v
Story loader ---- Blorb extraction and Glulx header/checksum validation
   |
   v
Vm -------------- memory, stack, decoder, instructions, minimal Glk dispatch
   |  input request / text output
   v
PlayerApp -------- window, transcript, options, file browser, lifecycle
   |
   +-------------- asynchronous OpenAI-compatible translation adapter
```

`Story` is a deep module around untrusted file parsing. Callers receive a
validated executable image and typed header instead of handling offsets.

`Vm` is the principal module. Its interface is intentionally limited to
construction, bounded execution, state inspection, text draining, input,
restart, and stop. Execution budgets keep the single-threaded GUI responsive.
Unsupported VM behavior is reported as a typed error with the program counter;
it never falls through to undefined behavior or `unimplemented!()`.

`PlayerApp` is an adapter at the presentation seam. It advances the VM in short
slices and converts `WaitingForLine`/`WaitingForChar` into controls. The VM has
no dependency on egui or HTTP.

`Translator` is another adapter. Completed narrative text is copied at the
input-wait boundary and sent to a worker thread. Translation cannot modify VM
memory, input, transcript data, or save state. Request IDs preserve turn order,
and the source string is the cache key.

## Reference Projects

Windows Git establishes the player behavior: startup file selection, story
extensions, restart/stop controls, scrollback, configurable text appearance,
and a status bar. Git also provides a mature reference for instruction and
Glk-dispatch behavior.

Gargoyle establishes the portability expectations: Unicode text, separate
platform adapters, asynchronous event handling, and broad Glk facilities. This
project uses eframe/egui to put that platform variation behind one Rust GUI
implementation.

No source from either project is copied into this repository.
