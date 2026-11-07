//! FastFAT: a GUI for formatting USB flash drives and HDDs as FAT32 and for
//! cloning whole disks.
//!
//! Long running work runs on a worker thread. Progress is published through
//! an atomic value that the GUI reads while repainting, so no messages or
//! allocations are needed per progress step.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod app;

#[cfg(windows)]
fn main() -> eframe::Result {
    app::run()
}

#[cfg(not(windows))]
fn main() {
    eprintln!("fastfat currently supports Windows only.");
}
