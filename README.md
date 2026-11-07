# FastFAT - Memory Formatter for Wii U

A Windows GUI tool (Rust / egui). The UI is available in Japanese and English (texts in `locales/*.json`).

## Build

```sh
cargo build --release
```

Formatting and cloning need administrator rights (the app asks for elevation through UAC at startup).

## FAT32 Format

- Formats drives larger than 32 GiB as FAT32 (up to 2 TiB with 512-byte sectors).
- Automatic or manual cluster size, volume label, quick or full format.
- "Wii U compatible": initialises the whole disk as MBR with a single FAT32 partition (the Wii U cannot read GPT).

## Disk Cloning

- Verbatim: sector-by-sector copy.
- Grow FAT32: copy to a larger disk and extend the FAT32 partition into the free space.
- For Wii U: grow FAT32 and create an empty Wii U storage partition at the end (the console formats it).

## Wii U FTP (read-only)

Two panes: the PC on the left, the Wii U on the right. Drag Wii U items onto the PC pane to download them.

- Shortcuts to console updates, games, the SD card and USB storage.
- Downloads whole folders or several selected items. Several connections can be used, but one is usually fastest: the console is the bottleneck.
- Runs in the background: browsing goes on, further downloads are queued.
- Shows the current file, its folder's progress, totals, speed and time left.
- Asks before touching files already on the PC: only new ones (same size = already downloaded), overwrite all, or cancel.
- Never sends FTP commands that write to the console.

Requirements:

- [ftpiiu](https://github.com/wiiu-env/ftpiiu_plugin)
  - download every modules (WiiUPluginLoaderBackend etc.)
  - To browse the internal memory, enable "Allow access to system files" in the plugin settings (L + D-pad Down + Minus).

## Code Layout

A Cargo workspace; the crates build in parallel and depend on each other in one direction only.

| Crate | Path | What it does |
|---|---|---|
| `fastfat` | `src/` | The GUI (egui): one file per page plus shared parts |
| `fastfat-device` | `crates/device` | Windows drives and disks: listing, raw access, formatting, cloning |
| `fastfat-clone` | `crates/clone` | Copying disks, partitions and folder trees |
| `fastfat-mbr` | `crates/mbr` | MBR partition tables |
| `fastfat-fat32` | `crates/fat32` | Writing a FAT32 file system (platform independent) |
| `fastfat-ftp` | `crates/ftp` | Read-only FTP downloads from the Wii U |
| `fastfat-units` | `crates/units` | Byte counts for display |

Inside the GUI (`src/app/`):

- `mod.rs`: window setup and the frame loop; `fonts.rs`, `i18n.rs` (texts), `dialog.rs`.
- `format_tab.rs`, `clone_tab.rs`, `ftp_tab.rs`: one file per page.
- `job.rs`: formatting and cloning on a worker thread.
- `browser/`: the two panes (PC on the left, Wii U on the right).
- `transfer/`: background downloads (queue, worker thread, progress, speed, progress panel).

Each file starts with a short comment that says what it is for.
