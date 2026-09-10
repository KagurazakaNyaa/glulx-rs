# Glulx Implementation Validation Record

[English](glulx-validation.md) | [中文](glulx-validation.ZH.md)

Updated: 2026-09-10. The current implementation state is commit `71e6889`; local validation was run on Linux x86_64. This record describes the current tree. It does not certify exhaustive Glulx, Glk, media, or platform conformance.

## Current Verification

```sh
RUSTC_WRAPPER= cargo fmt --all -- --check
RUSTC_WRAPPER= cargo test --all-targets
RUSTC_WRAPPER= cargo clippy --all-targets -- -D warnings
RUSTC_WRAPPER= cargo build --release
```

The current checks pass:

- 278 library tests pass; 6 manual performance tests remain ignored.
- 6 CLI tests pass.
- Formatting, all-target Clippy, and the release build pass.
- The undo regression proves that a 2 MiB story can retain a one-page undo snapshot under a 1 MiB undo budget; the shared story image is not charged to that snapshot budget.
- The bounded-page regression rejects dense dirty memory at the page limit before materializing the complete candidate page table.
- Dispatch tables contain all 150 official Glulx opcodes and all 124 official Glk selectors. Table completeness is not a substitute for semantic coverage.

The repository's repeatable microbenchmark suite covers instruction dispatch, linear search, text output, dirty-page snapshots, text layout, and CPU canvas rasterization:

```sh
RUSTC_WRAPPER= python3 tools/benchmark-engine.py --output "<output-dir>/glulx-engine-benchmark.json"
```

For large real stories, the separate tool measures startup and Linux peak RSS/HWM without writing beside the input stories:

```sh
python3 tools/benchmark-stories.py \
  "<story-dir>/story-a.gblorb" \
  "<story-dir>/story-b.gblorb" \
  --output "<output-dir>/glulx-real-story-baseline.json"
```

The real-story measurement is a startup/resident-memory workload, not a complete playthrough. The largest remaining memory costs are the story container, executable image, VM memory, and media/resource payloads. The repository does not currently use mmap or a block compiler.

The current release measurement recorded peak HWM of `755844 KiB` and `819592 KiB` for representative 650 MB and 705 MB Blorb files. These numbers are machine- and story-dependent and are retained as comparison evidence, not as a universal limit.

## Coverage Matrix

The main local evidence is in [conformance.rs](../src/vm/conformance.rs); implementation limits are summarized in [compatibility](compatibility.md).

| Domain | Current coverage |
| --- | --- |
| VM core | Header/checksum/layout validation, addressing, stack/locals, calls, strings, search, integer and floating-point instructions, heap, acceleration, verify, restart, protect, and typed failures. |
| Portable saves | IFZS CMem/UMem, Stks, MAll, identity checks, corrupt-input rejection, repeated annotations/extensions, and bidirectional Glulxe/Adventure interoperability fixtures. |
| Undo and sessions | 256-byte dirty-page snapshots, shared unchanged pages, restart/protection restoration, payload-based budget accounting, legacy session migration, and invalid-page rejection. |
| Glk | Window trees, streams, files, memory/resource streams, Unicode, input requests, timers, hyperlinks, styles, dates, images, sound channels, and 124-selector dispatch. |
| Resources | Bundled and external resource-only Blorb archives, loose resource directories, IFhd identity, discovery precedence, metadata, cover art, RDes, session retention, and resource cache reset. |
| Media | PNG/JPEG, AIFF/OGG/MP3, MOD/XM/S3M/IT, optional SONG resources, image scaling/clipping, streaming resampling, repeat/offset behavior, and software sample-frame synchronization. |
| Terminal | Interactive TTY grids/status, prefilled editing, timed cancellation, immediate character input, echo/terminators, file prompts, multi-window selection, and terminal restoration. Pipe mode remains a separate text/file automation protocol. |
| GUI | Linux Xvfb/software-OpenGL checks cover text layout, CJK fallback, styles, grid editing, image wrapping, hyperlinks, media completion, graphics resize, and session restoration. |

The matrix describes covered behavior and regression entry points; it does not claim every valid/invalid combination has been exercised.

## Reference Fixtures

Reference implementation revisions are Glulxe `56ab8743bab565de307bd892c555d8d8897ed517` and CheapGlk `14d8aaf6e4150669762bd4646a5368e75c1eeee6`. Fixtures come from the official Glulx fixture page or IF Archive and are not distributed with this repository. Commands accept their locations through `--fixtures`, as shown above and in the [checklist](glulx-spec-checklist.md).

The maintained fixture set covers Glulxercise, Unicode, resource streams, Adventure, Sensory Jam, input extensions, date/time, and multi-window startup. Exact fixture hashes and reference command options belong in the test run artifact, not in a machine-specific repository path.

## Platform Status

| Scenario | Linux x86_64 | Windows | macOS |
| --- | --- | --- | --- |
| VM, CLI, tests, Clippy, release build | Complete | CI/build coverage; on-device behavior pending | CI/build coverage; on-device behavior pending |
| GUI, fonts, DPI, native dialogs | Linux local validation | On-device validation pending | On-device validation pending |
| TTY and pipe hosts | Complete locally | Console validation pending | TTY validation pending |
| Physical audio output | Software sample-frame evidence; device waveform validation pending | Pending | Pending |
| Complete long-game walkthrough | Pending | Pending | Pending |

New validation results must record the platform, build commit, toolchain, input route, and failure behavior before changing this table.

## Reproduction Tools

Synthetic media, style, graphics, resource, terminal, SONG, and codec checks use repository scripts. Generated fixtures and output should go to a temporary or explicitly supplied output directory:

```sh
python3 tools/make-media-fixture.py "<output-dir>/glulx-media.gblorb"
python3 tools/make-style-fixture.py "<output-dir>/glulx-styles.ulx"
python3 tools/check-graphics-ui.py --candidate target/debug/glulx-rs --output "<output-dir>/glulx-graphics-ui"
python3 tools/check-terminal.py --candidate target/debug/glulx-rs
```

Reference checks require separately supplied fixtures and interpreter paths. They must not rely on a developer-specific absolute path.
