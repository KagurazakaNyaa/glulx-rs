//! Native font and file selection, with manual paths available on every platform.
#[cfg(any(windows, test))]
mod font_data;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;
#[cfg(target_os = "linux")]
pub(super) use linux::{choose_family, choose_file, font_bytes};
#[cfg(target_os = "macos")]
pub(super) use macos::{choose_family, choose_file, font_bytes};
#[cfg(windows)]
pub(super) use windows::{choose_family, choose_file, font_bytes};
