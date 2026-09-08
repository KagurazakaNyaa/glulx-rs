# Glulx Interpreter and GUI Technology Research

[English](interpreter-research.md) | [中文](interpreter-research.ZH.md)

Research date: 2026-09-08. Sources are limited to projects' own repositories, READMEs, specification sites, and crates.io registry records. Maintenance status is based on the latest commit/release records.

## Findings

Mature Glulx interpreters exist, but no ready-made project was found that combines active maintenance, pure Rust, complete Glulx, embeddability, and a native cross-platform GUI.

This project should implement its own pure Rust VM and Glk abstraction, while using established implementations alongside the specification:

1. Use **Glulxe** as the behavioral correctness reference and differential-test oracle.
2. Consult **Git** for decoding, execution loops, caching, and acceleration strategies; avoid copying its dynamic-compilation complexity into the first version.
3. Consult **Gargoyle** for the interpreter-to-GUI boundary through Glk, rather than embedding Gargoyle source directly into the Rust application.
4. Consult **Quixe/GlkOte** for browser window trees, events, and save interactions; it suits Web targets, not a native Rust VM core.
5. Use **RemGlk / remglk-rs** as adapters for headless tests, remote frontends, or Web services; they are neither VMs nor GUIs.
6. Do not build on `thefarwind/glulx-rs`. It has long been inactive and explicitly cannot yet run story files; selectively consult its testing ideas instead.

If the short-term goal is to run real games quickly, an optional C Glulxe backend could serve as a development baseline, while the product remains centered on a pure Rust VM. The crates.io `glulxe`/`glulxe-sys` packages provide this bridge, but their versions date to 2019 and should not form the long-term architectural core.

## Candidate Comparison

| Project | Role and language | GUI / I/O | Maintenance signals | License | Value to this project |
| --- | --- | --- | --- | --- | --- |
| [Git](https://github.com/DavidKinder/Git) | High-speed Glulx interpreter in C | No GUI itself; must link Glk; repository includes Windows Glk build support | Commits as recent as 2026-08-23; repository not archived | MIT | Performance/executor design reference and differential testing; not directly a pure Rust core |
| [Glulxe](https://github.com/erkyrath/glulxe) | Glulx reference interpreter in C | No GUI itself; must link Glk, such as RemGlk or CheapGlk | Commits as recent as 2026-05-05; README documents 3.1.3 double-precision/undo support | MIT | Most trustworthy compatibility baseline; oracle or optional FFI backend |
| [Quixe](https://github.com/erkyrath/quixe) | JavaScript Glulx VM running entirely in a browser | Includes GlkOte browser UI; text/grids, input, timers, links, and experimental graphics; README explicitly lists sound and style hints as unsupported | 2.2.6 released 2025-06-02; latest commit 2025-09-01 | MIT (bundled build tools also include Apache-2.0) | Complete Web implementation reference and UI behavior examples; not a Rust/native reusable core |
| [RemGlk](https://github.com/erkyrath/remglk) | C Glk RPC implementation, not a VM | No UI; JSON over stdout/stdin for GlkOte, bots, Web services, or regression tests | 0.3.2; latest commit 2025-06-12 | MIT; documentation CC BY-NC-SA 4.0 | Excellent automation/remote-frontend protocol reference; cannot replace a GUI |
| [Gargoyle](https://github.com/garglk/garglk) | C/C++ cross-platform IF player and Glk implementation bundling multiple interpreters | Qt 5/6 or macOS Cocoa; full desktop images, sound, fonts, scrollback, file dialogs, and more | Commits and multi-platform CI as recent as 2026-09-07 | Main README declares GPL; bundled interpreters retain their own licenses | Most complete desktop integration reference; broad dependency/license scope makes direct transplantation into a lightweight Rust GUI unsuitable |
| [`glulxe` crate](https://crates.io/crates/glulxe) / [`glulxe-sys`](https://crates.io/crates/glulxe-sys) | Rust API plus embedded C Glulxe FFI | Caller provides Glk handlers; `main()` takes over the current thread until completion | Latest 0.2.0, released 2019-11-04 | Wrapper MIT OR Apache-2.0; inner C/`-sys` MIT | Development oracle/compatibility backend; aging API, upstream snapshot, and maintenance |
| [`glk` crate](https://crates.io/crates/glk) | Rust traits for a Glk provider called by C interpreters | No GUI; example is a single-window terminal ToyGlk | Latest 0.2.0, released 2019-11-04; docs target Glk 0.7.5 | MIT OR Apache-2.0 | Trait/FFI mapping reference; unsuitable for fixing the long-term UI API around it |
| [`thefarwind/glulx-rs`](https://github.com/thefarwind/glulx-rs) | Early pure Rust Glulx experiment | No finished GUI/Glk | Last push 2018-02-10; only a few VM/memory/stack files | MIT | Not a baseline; readable reference, not a fork to continue |
| [`curiousdannii/remglk-rs`](https://github.com/curiousdannii/remglk-rs) | Active Rust RemGlk/GlkOte protocol implementation, not a Glulx VM | `GlkSystem` trait sends updates and receives events; includes Blorb and C API layers | Latest commit 2026-08-24; workspace version 0.1.0 | MIT | Most promising Rust Glk protocol component to consult/reuse; native GUI still needs implementation |

## Individual Analysis

### Git

Git targets speed. Its official README claims roughly five times Glulxe's speed, with configurable cache sizes trading memory for speed. The source tree separates `compiler.c`, `peephole.c`, `opcodes.c`, `operands.c`, `memory.c`, `savefile.c`, and `glkop.c`, making it useful for studying executor responsibilities. [README](https://github.com/DavidKinder/Git/blob/master/README.md) [source tree](https://github.com/DavidKinder/Git)

It is not a GUI program. The README explicitly requires linking a Glk library, with only additional Windows Glk build materials in the repository. MIT permits adaptation or porting, while copied code still requires copyright and license notices. [LICENSE](https://github.com/DavidKinder/Git/blob/master/LICENSE)

Recommendation: implement a direct interpreter and establish correctness through tests first; introduce a decoded-block cache based on profiling. Porting Git's code generator during the MVP would significantly increase unsafe code, platform differences, and debugging costs.

### Glulxe

Glulxe describes itself as “The Glulx VM reference interpreter”. Its README says it must link a Glk library and can use CheapGlk, GlkTerm, RemGlk, and other backends. This confirms that separating VM and display is an established Glulx ecosystem boundary. [README](https://github.com/erkyrath/glulxe/blob/master/README.md)

Version history shows 0.6.0 already supported Glulx 3.1.3's `hasundo`, `discardundo`, and double-precision instructions. Later master added autosave/restoration; the 2026-05-05 commit still addresses extended-memory mapping, supporting its role as an active oracle covering edge behavior. [latest commit](https://github.com/erkyrath/glulxe/commit/56ab8743bab565de307bd892c555d8d8897ed517) [LICENSE](https://github.com/erkyrath/glulxe/blob/master/LICENSE)

Recommendation: run identical `.ulx` input sequences in the Rust VM and Glulxe/RemGlk, comparing JSON output, exit status, and save files. Glulxe C source should remain a reference or isolated FFI feature, not a VM-core dependency.

### Quixe and GlkOte

Quixe is a pure JavaScript Glulx VM that runs `.ulx` or `.gblorb` in a browser without a server. It separates the VM core, Glk dispatcher, story/Blorb loader, and GlkOte UI, offering a useful Web module boundary reference. [Quixe README](https://github.com/erkyrath/quixe/blob/master/README.txt)

However, its official README still explicitly lists sound and style hints as unsupported; playing most games in a browser is not complete Glk coverage. A future Web/WASM frontend could reuse its interaction model or map Rust VM output to GlkOte. The native desktop MVP does not need a JavaScript runtime. [Quixe LICENSE](https://github.com/erkyrath/quixe/blob/master/LICENSE) [GlkOte README](https://github.com/erkyrath/glkote/blob/master/README.txt)

### RemGlk and remglk-rs

RemGlk is a structured I/O backend: the interpreter encodes window changes as JSON on stdout and reads JSON events from stdin. It supports multiple windows and most Glk I/O, but its documentation repeatedly emphasizes that it “does not provide a user interface”. [RemGlk README](https://github.com/erkyrath/remglk/blob/master/README.txt) [protocol documentation](https://github.com/erkyrath/remglk/blob/master/docs.html)

`remglk-rs` actively implements this approach in Rust. Its `GlkSystem` trait abstracts file operations, GlkOte updates/events, Unicode, and time/directory services; the source tree also includes Blorb, protocol objects, windows/streams/sound channels, and a C API. [README](https://github.com/curiousdannii/remglk-rs/blob/master/README.md) [`GlkSystem`](https://github.com/curiousdannii/remglk-rs/blob/master/remglk/src/lib.rs) [Cargo manifest](https://github.com/curiousdannii/remglk-rs/blob/master/remglk/Cargo.toml)

Recommendation: define a small project-owned `GlkHost`/event interface rather than coupling the VM directly to RemGlk JSON; RemGlk, GUI, and headless recorder are adapters. A git dependency can trial `remglk-rs`, but while its public API is unsettled and no formal crates.io entry was found, its core types should not leak into the VM API.

### Gargoyle

Gargoyle is a complete cross-platform IF player, not a single Glulx VM. Its README lists bundled interpreters including Git and Glulxe. Build files compile each interpreter as a separate executable linked to `garglkmain` and `garglk`. This provides concrete design evidence: GUI/Glk forms the platform layer, while Git/Glulxe are replaceable engines. [README](https://github.com/garglk/garglk/blob/master/README.md) [interpreter build](https://github.com/garglk/garglk/blob/master/terps/CMakeLists.txt)

The current GUI uses Qt Widgets outside macOS and can use Cocoa on macOS. Sound can use Qt, SDL2, SDL3, or be disabled; JPEG/PNG, fonts, TTS, and other facilities are handled separately. Copying it wholesale would add substantial native dependencies and multiple license obligations, but its window trees, layout, scrollback, input history, fullscreen, themes, and accessibility are valuable acceptance references. [GUI build](https://github.com/garglk/garglk/blob/master/garglk/CMakeLists.txt)

This repository is `AGPL-3.0-only`. MIT code from Git/Glulxe/remglk-rs can generally be incorporated with notices retained, but GPL version differences across Gargoyle and its bundled components require per-file auditing. This section records engineering risk, not legal advice. [Gargoyle licensing statement](https://github.com/garglk/garglk/blob/master/README.md) [Gargoyle license inventory](https://github.com/garglk/garglk/tree/master/licenses)

### Existing Rust Implementations

crates.io search mainly returns two categories: Glulxe C FFI (`glulxe`, `glulxe-sys`) and Glk providers (`glk`, `glk-sys`), rather than a complete pure Rust VM. The official `glulxe` crate README explicitly says it embeds C Glulxe; after `init()`, `main()` takes over execution on the same thread. The GUI must still provide all Glk handlers. [`glulxe` crate README](https://crates.io/crates/glulxe/0.2.0) [`glk` crate README](https://crates.io/crates/glk/0.2.0)

The old `thefarwind/glulx-rs` stopped receiving updates in 2018. Its source contains multiple `unimplemented!()` calls, and the `glulxe` crate README records that it was still insufficient to run story files in late 2019. It cannot be treated as an available interpreter that shortens delivery. [repository](https://github.com/thefarwind/glulx-rs) [interpreter source](https://github.com/thefarwind/glulx-rs/blob/master/src/interpreter.rs)

Keyword searches also return non-Glulx-VM projects, such as `glulx-asm`/`wasm2glulx` (which generate Glulx) and the recently released `rezrov` (whose workspace currently declares only `rezrov-zterp`, a Z-machine). These do not satisfy this project's needs. [crates.io search](https://crates.io/search?q=glulx) [Rezrov manifest](https://github.com/jeffnyman/rezrov/blob/main/Cargo.toml)

## Concrete Architecture Recommendations

```text
glulx-core (pure Rust, no GUI/file dialogs)
  Story/Header -> Memory -> Decoder -> Executor -> Save/Undo
                                     |
                                     v
                               GlkHost trait
                              /      |       \
                    eframe adapter  headless  RemGlk adapter
                         |           recorder     (optional)
                    desktop GUI       |
                                   differential tests
                                      |
                             Glulxe + RemGlk oracle
```

Boundaries should follow these rules:

- The VM knows Glk selectors, arguments, and asynchronous events, not eframe, Qt, JSON, widgets, or operating-system file dialogs.
- The GUI maintains the Glk window tree, text/grid buffers, input requests, image/sound resources, and menu state; the VM thread can pause while awaiting a Glk event without blocking rendering.
- Story/header/memory/operand/opcode layers should follow the official Glulx specification and gradually cover 3.1.3 features. Official entry points: [Glulx home/specification](https://eblong.com/zarf/glulx/) and [Glk home/specification](https://eblong.com/zarf/glk/).
- MVP priorities: `.ulx`/`.gblorb` loading, validation, integer execution, call stack, string I/O, core Glk text windows/input, and quit/restart/save/restore/undo. Add float/double, heap, search, acceleration, graphics, sound, and complete style hints afterward.
- Keep deterministic RNG, step limits, checked address arithmetic, and structured errors from day one. Glulxe/Git continually fix edge behavior; real story files cannot be treated as trusted input.

## Recommended Reuse List

**Reuse directly**

- Glulx/Glk specifications as the sole behavioral contract.
- Glulxe, Git, and Quixe test stories and public behavior for differential validation, checking each resource's license.
- `remglk-rs` protocol/Blorb ideas; decide whether to depend on it after a small spike.

**Reference or development tools only**

- Glulxe C / `glulxe` crate: oracle, compatibility backend, fault isolation.
- Git: performance optimization and module responsibilities.
- Gargoyle: Glk capability matrix, desktop interactions, and release matrix.
- Quixe/GlkOte: future Web/WASM frontend.

**Do not use as the foundation**

- `thefarwind/glulx-rs`: incomplete and long inactive.
- A direct Gargoyle fork: this would turn the Rust VM project into C/C++/Qt integration.
- RemGlk as a GUI: it defines only a structured I/O channel.
- Git-style dynamic compilation in the MVP: low benefit and high risk before compatibility tests mature.

## Primary Source Index

- Glulx/Glk: [Glulx](https://eblong.com/zarf/glulx/), [Glk](https://eblong.com/zarf/glk/)
- Git: [repository](https://github.com/DavidKinder/Git), [README](https://github.com/DavidKinder/Git/blob/master/README.md), [LICENSE](https://github.com/DavidKinder/Git/blob/master/LICENSE)
- Glulxe: [repository](https://github.com/erkyrath/glulxe), [README](https://github.com/erkyrath/glulxe/blob/master/README.md), [LICENSE](https://github.com/erkyrath/glulxe/blob/master/LICENSE)
- Quixe/GlkOte: [Quixe](https://github.com/erkyrath/quixe), [GlkOte](https://github.com/erkyrath/glkote)
- RemGlk: [C implementation](https://github.com/erkyrath/remglk), [Rust implementation](https://github.com/curiousdannii/remglk-rs)
- Gargoyle: [repository](https://github.com/garglk/garglk), [README](https://github.com/garglk/garglk/blob/master/README.md)
- Rust registry: [glulxe](https://crates.io/crates/glulxe), [glulxe-sys](https://crates.io/crates/glulxe-sys), [glk](https://crates.io/crates/glk), [glk-sys](https://crates.io/crates/glk-sys)
