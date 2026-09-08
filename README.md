# glulx-rs

A pure Rust Glulx virtual machine with a cross-platform graphical player.

The VM follows the Glulx specification and uses
[David Kinder's Git](https://github.com/DavidKinder/Git) as its behavioral
reference. The desktop workflow follows Windows Git: open a story, play in a
single main window, inspect scrollback, restart or stop the VM, and configure
fonts and colors. [Gargoyle](https://github.com/garglk/garglk) is used as a
reference for portable Glk behavior rather than for the visual design.

This repository is an early executable milestone, not yet a drop-in replacement
for Git or Glulxe. See [docs/compatibility.md](docs/compatibility.md) for the
exact implemented surface.

## Run

```sh
cargo run --release
```

Open `.ulx`, `.blb`, `.blorb`, `.glb`, or `.gblorb` from the player. A story can
also be passed on the command line or dropped onto the window:

```sh
cargo run --release -- path/to/story.gblorb
```

For automated checks or systems without a display server, use the terminal
adapter compiled into the same executable:

```sh
cargo run --release -- --headless path/to/story.ulx
```

## Translation

Turn translation is optional and disabled by default. Enable it from
`View -> Translation panel`, then configure an OpenAI-compatible endpoint,
model, target language, and system prompt in `View -> Options`.

Text is collected from narrative output and submitted asynchronously when the
VM waits for player input. The original text remains authoritative and appears
immediately. Player commands are never translated. Results are ordered by turn
and repeated passages use an in-memory cache.

The default model name is `tencent/Hy-MT2-1.8B`; any compatible local or remote
endpoint can be used. Credentials remain in the desktop application's local
settings and are never embedded in release artifacts.

## Build And Test

```sh
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo build --release
```

The release profile uses LTO and strips symbols. Tagged GitHub releases build
native Linux, macOS, and Windows artifacts. The Windows artifact is one
`glulx-rs.exe`; the VM, GUI, TLS client, and translation integration are all
compiled into it.

## Design

The core seam is deliberately small: `Story` validates and extracts an image,
`Vm` accepts input and produces text while exposing only its run state, and the
desktop player owns presentation and translation. See
[docs/architecture.md](docs/architecture.md).

The survey of existing native, browser, C, and Rust interpreters is recorded in
[docs/interpreter-research.md](docs/interpreter-research.md).

Licensed under AGPL-3.0-only.
