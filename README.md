# glulx-rs

[English](README.md) | [中文](README.ZH.md)

A pure Rust Glulx virtual machine with a cross-platform graphical player.

The VM follows the Glulx specification and uses
[David Kinder's Git](https://github.com/DavidKinder/Git) as its behavioral
reference. The desktop workflow follows Windows Git: open a story, play in a
single main window, inspect scrollback, restart or stop the VM, and configure
fonts and colors. [Gargoyle](https://github.com/garglk/garglk) is used as a
reference for portable Glk behavior rather than for the visual design.

The player implements Glulx 3.1.3 and Glk 0.7.6 with explicitly bounded optional
capabilities; it is not yet a drop-in replacement for every Git or Glulxe workflow. See [docs/compatibility.md](docs/compatibility.md) for the
exact implemented surface.

The specification-based [compatibility checklist](docs/glulx-spec-checklist.md)
tracks implemented features, remaining gaps, and validation work against the
public Glulx, Glk, and Blorb specifications.

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

The desktop uses three native windows: the game canvas, **Log and input**, and
**Translation**. Resize or close companion windows without resizing the game.
Use the toolbar Log and Translation toggles to show or hide them. Settings opens
its own native window. View and Ctrl+Shift+L / Ctrl+Shift+T can also reopen companions.
Game frames are published at Glk event boundaries, preserving the previous complete
frame while the next is composed.

Game-requested save/load prompts accept a path in the input bar; existing files
require confirmation before modification, and read prompts require an existing file. The desktop
also saves its session every 30 seconds and on normal exit; launching without a
story restores the previous session. Large story/memory/canvas payloads (over 16 MiB)
skip automatic session snapshots to keep the UI responsive; use the game’s Save command
for those stories. Settings persist in `glulx-settings.json` beside the executable. The JSON file
takes priority over old eframe settings; the first run without JSON migrates them.
Use Save settings or close the Settings window to write changes immediately;
settings also save every 30 seconds and on normal exit. Manual file edits take
effect after restarting. Desktop sessions remain in eframe storage. `View -> Story information` shows available
iFiction metadata, cover art and image/sound text descriptions. Portable game saves and desktop sessions are
separate formats.

The desktop supports paragraph alignment and indentation, styled grids with inline
editing, inline and margin images with resizing, picture hyperlinks,
and MOD/XM/S3M/IT music alongside sampled audio. Inform acceleration functions 1–13 are
available. Settings supports native font selection on Windows/macOS and GTK 3 desktops,
or a TTF/OTF/TTC file. The selected face takes priority in its matching proportional or monospace family.
Fixed-pitch/HW faces remain fallback fonts for missing proportional glyphs rather than replacing proportional Latin body text. See the compatibility document for tested media formats and limits.

External resources can be selected before startup:

```sh
cargo run --release -- --resources path/to/media.blorb path/to/story.ulx
cargo run --release -- --resources path/to/media-directory path/to/story.ulx
cargo run --release -- --no-auto-resources path/to/story.ulx
```

The File → Choose resources dialog applies a selection by restarting the story.
Interactive `--headless` sessions display grids and support live line editing and
single-key input; pipes retain the automation protocol. In a terminal, Ctrl+N changes
input windows and Ctrl+C exits. SONG audio and available light font faces are supported.

The interface supports English and Chinese. **Settings → Interface language** defaults to **Follow system**; unsupported or unavailable system locales fall back to English. Selecting **English** or **中文** takes effect immediately and is saved as `language` (`auto`, `en`, or `zh`) in `glulx-settings.json`. Interface language is independent of story translation. Native system dialog controls follow the operating system language. Translation catalogs and contributor instructions: [assets/locales](assets/locales/README.md).

## Performance and memory limits

Use release or diagnostic builds for gameplay performance. Operand decoding and search comparisons
now avoid temporary heap allocations. The repository provides two fixed microbenchmarks: five million
branch dispatches, 1,000 linear searches through 16,384 records, 4/8/16 KiB text output,
sparse/dense dirty-page snapshots on 64 MiB memory, 16 KiB cold/warm text layout, and
512x384 CPU canvas rasterization. The benchmark tool records the
commit, platform, Rust version, total time and time per operation; these metrics do not predict whole-game speedups.

Every budget in Settings supports **fixed MiB** or **1%–100% of startup memory**, saved beside the executable in `glulx-settings.json`. New configurations use these fixed defaults; existing numeric settings retain their fixed-MiB meaning:

| Setting | Default MiB | What is counted |
| --- | ---: | --- |
| Game memory | 1024 | VM address space including the game heap; excludes stack, original story copies and undo; capped by Glulx at 4 GiB minus 256 bytes |
| Undo snapshots | 256 | Retained memory, initial images, stacks and heap-record payloads; oldest snapshots are evicted; an oversized snapshot fails |
| Graphics image cache | 512 | Cached RGBA and texture pixel payloads, evicted by least recent use |
| Text image cache | 256 | Cached texture pixel payloads, evicted by least recent use |
| Each decoded image | 256 | RGBA output for one picture; oversized resources fail |
| Each encoded audio resource | 256 | Encoded bytes of each playback resource and SONG reference, not total decoder memory |
| SONG sample data per playback | 128 | Unique PCM sample payloads for one playback |
| Process hard limit | 0 | 0 adds no limit; committed process memory on Windows, virtual address space on Linux |

Category budgets are not a total process limit. Displayed images can remain referenced by canvases;
drivers, decoders, containers and allocators consume additional memory. Zero disables cache/undo
retention; zero image/audio budgets make those resources unavailable. Game, undo and media limits
apply on the next game open or session restore; the process limit requires an **application restart**.
Saves and sessions exceeding the configured game limit are rejected; saves cannot override it.

All percentages share one memory snapshot captured at startup; they do not fluctuate during play:

- With cgroup v1/v2 limits, Linux uses the smallest finite **total limit** in the current group and visible ancestors, without subtracting usage.
- Without a cgroup limit, Linux uses `/proc/meminfo`'s `MemAvailable`.
- Windows uses available physical memory (`GlobalMemoryStatusEx.ullAvailPhys`).

Settings shows the source, base and resolved budget. Game limits round down to 256-byte alignment and the Glulx address-space maximum; resource budgets round down to MiB; process budgets use bytes.
Resources no longer have a blanket 4095 MiB ceiling. SONG uses the configured audio-resource budget instead of a separate 1 MiB file limit.
If detection fails, fixed limits still work and percentage limits report an error. Percentage detection is currently unsupported on macOS.

**CLI > configuration file > defaults** applies in both GUI and terminal modes. CLI overrides affect only the current run and never rewrite the settings file. Every memory argument accepts fixed MiB or a percentage:

```sh
cargo run --release -- --max-memory 25% --max-process-memory 75% --max-undo-memory 10% path/to/story.gblorb
cargo run --release -- --headless --max-memory 2048 --max-graphics-cache 8192 path/to/story.gblorb
```

Other flags are `--max-text-image-cache`, `--max-decoded-image`, `--max-audio-resource` and `--max-song-pcm`.
JSON can mix numeric and percentage settings:

```json
{
  "max_memory_mib": {"percent": 25},
  "max_process_memory_mib": {"percent": 75},
  "resource_limits": {
    "undo_mib": {"percent": 10},
    "graphics_cache_mib": 512
  }
}
```

Linux uses `RLIMIT_AS`, including mappings, shared libraries and reserved thread stacks; it is not
an RSS limit. Windows uses a Job Object process commit limit. A nonzero process limit is currently
unsupported on macOS and produces an error. Failure to install an OS limit aborts startup rather
than silently ignoring it. A limit that is too low may prevent startup or fail subsequent allocations
without a chance to save progress. Recover with `--max-process-memory 0` or edit the JSON file.
Stricter inherited system limits remain in effect.

Run the microbenchmarks and write JSON evidence with:

```sh
python3 tools/benchmark-engine.py --output "<output-dir>/glulx-engine-benchmark.json"
```

The tool uses release mode, one test thread and the `benchmark_` ignored tests. For comparisons, keep
the machine, power mode, toolchain and working-tree commit fixed.

For real stories, use the separate tool to measure startup time and peak Linux RSS/HWM. It uses a
temporary working directory and does not write beside the stories:

```sh
python3 tools/benchmark-stories.py \
  "<story-dir>/story-a.gblorb" \
  "<story-dir>/story-b.gblorb" \
  --output "<output-dir>/glulx-real-story-baseline.json"
```

## Translation

Turn translation is optional and disabled by default. Enable it from
`View -> Translation window`, enable translation there, then configure an OpenAI-compatible endpoint,
model, target language, prompts, and sampling parameters using **Translation settings…**
in the translation window. These controls open in a dedicated window, separate
from general settings. Both settings windows save when closed.

Text is collected from narrative output and submitted asynchronously when the
VM waits for player input. The original text remains authoritative and appears
immediately. Player commands are never translated. Results are ordered by turn
and repeated passages use a session cache scoped to the translation settings.
Concurrent identical requests are merged. The switch controls new output only:
it never backfills old paragraphs or clears existing results, and submitted
requests can finish after it is switched off. Current source and translation are
shown directly; older views are under a collapsed History control. Output appends
to the current view until the game clears its text buffer. Identical redraws do
not add duplicate history, and text cleared before submission is discarded.

Configure the endpoint, model identifier, prompts, and sampling parameters for
any model served through a compatible Chat Completions API, including HY-MT2,
DeepSeek, and GPT models. Follow the selected model's supported roles and
parameters; there are no model-specific presets or automatic overrides.

System messages can be disabled or left blank. User templates support `{target}`
and `{text}`; without `{text}`, the source is appended after a blank line.
An empty user template sends the source unchanged. Uncheck any sampling parameter
to omit it and use the server default; `top_k` and `repetition_penalty` require
endpoint support. Maximum output tokens uses the `max_tokens` API field.
Existing settings retain system messages, raw user text, and temperature 0.2
until changed. Prompt and sampling changes are included in the session cache key.

The default model name is `tencent/Hy-MT2-1.8B`; any compatible local or remote
endpoint can be used. Credentials remain in the desktop application's local
settings and are never embedded in release artifacts.

## Build And Test

Building from source requires Rust 1.95 or newer. CI uses the latest stable Rust;
Cargo.toml declares compatible major/minor ranges and Cargo.lock records the exact
validated versions. Runner and GitHub Action choices are explained inline in
[the release workflow](.github/workflows/release.yml).

Linux builds require ALSA development headers (`libasound2-dev` on Debian/Ubuntu),
in addition to the usual graphical build dependencies.

```sh
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo build --release
```

Optional reference validation (download fixtures separately; see
[validation record](docs/glulx-validation.md)):

```sh
python3 tools/check-reference.py --reference /path/to/glulxe --candidate target/debug/glulx-rs --fixtures /path/to/fixtures
```

An original media fixture can be generated locally to inspect image wrapping,
clickable pictures, window resizing and MOD playback:

```sh
python3 tools/make-media-fixture.py "<output-dir>/glulx-media.gblorb"
cargo run -- "<output-dir>/glulx-media.gblorb"
```

Paragraph styles and grid interaction have a separate original fixture:

```sh
python3 tools/make-style-fixture.py "<output-dir>/glulx-styles.ulx"
cargo run -- "<output-dir>/glulx-styles.ulx"
```

Linux desktop input and graphics regression tools use an isolated Xvfb display:

```sh
python3 tools/check-input-ui.py --candidate target/debug/glulx-rs
python3 tools/check-graphics-ui.py --candidate target/debug/glulx-rs
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

Graphics canvases use GPU texture scaling and blending on hardware OpenGL. Software OpenGL uses a cached CPU canvas; diagnostics report the renderer and active path.
