//! The Wii U over LAN page: browsing the console through the `ftpiiu`
//! plugin and downloading to the PC (read-only).

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use eframe::egui;
use fastfat_ftp::{self as ftp, FtpClient, FtpEntry, FtpEntryKind, FtpError};
use fastfat_units::format_size;

use super::browser::{self, Action, Row, RowKind};
use super::i18n::{t, tf};
use super::transfer::TransferRequest;
use super::{FastFatApp, Status};

/// A connection shared with download jobs.
pub(super) type SharedFtp = Arc<Mutex<FtpClient>>;

/// Shortcuts shown above the listing: (label, path).
const SHORTCUTS: [(&str, &str); 5] = [
    ("ftp.shortcut_root", "/"),
    ("ftp.shortcut_sd", "/fs/vol/external01"),
    ("ftp.shortcut_updates", "/storage_mlc/usr/title/0005000e"),
    ("ftp.shortcut_games", "/storage_mlc/usr/title/00050000"),
    (
        "ftp.shortcut_usb_updates",
        "/storage_usb/usr/title/0005000e",
    ),
];

/// Result of a background listing.
type Listing = Result<Vec<FtpEntry>, String>;

/// Connections used for downloads by default.
const DEFAULT_WORKERS: usize = 1;

/// State of the FTP page.
pub(super) struct FtpState {
    /// Address of the console, e.g. `192.168.0.10`.
    host: String,
    /// The connection.
    pub(super) client: Option<SharedFtp>,
    /// Directory shown.
    path: String,
    /// Entries of the directory shown.
    entries: Vec<FtpEntry>,
    /// Check marks of the entries.
    selected: Vec<bool>,
    /// Connections used for downloads.
    workers: usize,
    /// Listing in progress: its path and the channel of its result.
    listing: Option<(String, Receiver<Listing>)>,
}

impl Default for FtpState {
    fn default() -> Self {
        Self {
            host: String::new(),
            client: None,
            path: "/".to_owned(),
            entries: Vec::new(),
            selected: Vec::new(),
            workers: DEFAULT_WORKERS,
            listing: None,
        }
    }
}

impl FastFatApp {
    /// Draws the FTP page.
    pub(super) fn ftp_section(&mut self, ui: &mut egui::Ui) {
        let dropped: Vec<PathBuf> = ui.ctx().input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| p.is_dir())
                .collect()
        });
        if let Some(folder) = dropped.last() {
            self.local.open(Some(folder.clone()));
        }
        self.poll_ftp_listing(ui.ctx());

        ui.colored_label(
            egui::Color32::from_rgb(0x2E, 0xA0, 0x43),
            t("ftp.read_only"),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(t("ftp.address"));
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.ftp.host)
                    .desired_width(180.0)
                    .hint_text(t("ftp.address_hint")),
            );
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let connected = self.ftp.client.is_some();
            let can_connect = !self.ftp.host.trim().is_empty();
            let label = if connected {
                t("ftp.reconnect")
            } else {
                t("ftp.connect")
            };
            let clicked = ui
                .add_enabled(can_connect, egui::Button::new(label))
                .clicked();
            if clicked || (enter && can_connect) {
                self.connect_ftp();
            }
            if connected && ui.button(t("ftp.disconnect")).clicked() {
                self.disconnect_ftp();
            }
            ui.separator();
            ui.label(t("ftp.workers"));
            ui.add(egui::DragValue::new(&mut self.ftp.workers).range(1..=ftp::MAX_WORKERS))
                .on_hover_text(t("ftp.workers_hint"));
        });
        ui.collapsing(t("ftp.setup_title"), |ui| {
            ui.label(t("ftp.setup1"));
            ui.label(t("ftp.setup2"));
            ui.label(t("ftp.setup3"));
            ui.label(t("ftp.setup4"));
            ui.label(t("ftp.setup5"));
            ui.label(t("ftp.setup6"));
        });

        if self.ftp.client.is_some() {
            ui.separator();
            self.ftp_browser(ui);
        }
    }

    /// Connects to the console and lists the root.
    fn connect_ftp(&mut self) {
        self.disconnect_ftp();
        match FtpClient::connect(&self.ftp.host) {
            Ok(client) => {
                self.ftp.client = Some(Arc::new(Mutex::new(client)));
                self.status = Status::Info(Cow::Borrowed(t("ftp.connected")));
                self.load_ftp_dir("/".to_owned());
            }
            Err(err) => {
                self.status = Status::Error(tf("ftp.connect_failed", &[("err", &err)]));
            }
        }
    }

    /// Closes the connection.
    fn disconnect_ftp(&mut self) {
        if let Some(client) = self.ftp.client.take() {
            if let Ok(mutex) = Arc::try_unwrap(client) {
                if let Ok(client) = mutex.into_inner() {
                    client.quit();
                }
            }
        }
        self.ftp.entries.clear();
    }

    /// Starts listing `path` in the background; it is shown when the
    /// listing succeeds (see [`Self::poll_ftp_listing`]).
    fn load_ftp_dir(&mut self, path: String) {
        let Some(client) = self.ftp.client.clone() else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let listed = path.clone();
        let spawned = std::thread::Builder::new()
            .name("ftp-list".to_owned())
            .spawn(move || {
                let result = match client.lock() {
                    Ok(mut client) => client.list(&listed).map_err(|e| e.to_string()),
                    Err(_) => Err("the connection lock is poisoned".to_owned()),
                };
                let _ = sender.send(result);
            });
        match spawned {
            // A newer listing replaces an older one still running.
            Ok(_) => self.ftp.listing = Some((path, receiver)),
            Err(err) => self.status = Status::Error(err.to_string()),
        }
    }

    /// Shows the result of the background listing once it is there.
    fn poll_ftp_listing(&mut self, ctx: &egui::Context) {
        let Some((path, receiver)) = &self.ftp.listing else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
                return;
            }
            Err(TryRecvError::Disconnected) => Err("the listing thread stopped".to_owned()),
        };
        let path = path.clone();
        self.ftp.listing = None;
        match result {
            Ok(entries) => {
                self.ftp.entries = entries;
                self.ftp.selected.clear();
                self.ftp.path = path;
            }
            Err(err) => {
                let hint = if path.starts_with("/storage_") {
                    t("ftp.system_files_hint")
                } else {
                    ""
                };
                self.status = Status::Error(format!("{path}: {err}{hint}"));
            }
        }
    }

    /// Queues a download of `entries` (in the directory `parent`) into the
    /// PC folder shown on the left.
    fn queue_download(&mut self, entries: Vec<FtpEntry>, parent: &str) {
        let Some(destination) = self.local.dir().map(Path::to_path_buf) else {
            self.status = Status::Error(t("pane.open_destination_left").to_owned());
            return;
        };
        if !destination.is_dir() {
            self.status = Status::Error(t("job.no_destination").to_owned());
            return;
        }
        if entries.is_empty() {
            return;
        }
        self.transfers.push(TransferRequest {
            host: self.ftp.host.trim().to_owned(),
            parent: parent.to_owned(),
            entries,
            destination,
            workers: self.ftp.workers,
        });
    }

    /// Draws the listing and the drop zone.
    fn ftp_browser(&mut self, ui: &mut egui::Ui) {
        let rows: Vec<Row<'_>> = self
            .ftp
            .entries
            .iter()
            .map(|entry| Row {
                name: &entry.name,
                kind: match entry.kind {
                    FtpEntryKind::Directory => RowKind::Directory,
                    FtpEntryKind::File => RowKind::File,
                    FtpEntryKind::Link => RowKind::Link,
                },
                size: entry.size,
            })
            .collect();
        let path = self.ftp.path.clone();
        let can_up = path != "/";
        let mut action = None;
        let mut dropped = None;
        ui.columns(2, |columns| {
            dropped = self.local.draw(&mut columns[0]);
            action = browser::draw_remote(
                &mut columns[1],
                "ftp",
                &path,
                can_up,
                &rows,
                &mut self.ftp.selected,
                &SHORTCUTS,
            );
        });
        drop(rows);
        if let Some(indices) =
            dropped.and_then(|d| browser::dropped_rows(d, "ftp", &self.ftp.selected))
        {
            action = Some(Action::Copy(indices));
        }
        match action {
            Some(Action::Up) => self.load_ftp_dir(ftp::parent(&path)),
            Some(Action::Goto(target)) => self.load_ftp_dir(target.to_owned()),
            Some(Action::Enter(index)) => {
                if let Some(entry) = self.ftp.entries.get(index) {
                    let child = ftp::join(&path, &entry.name);
                    self.load_ftp_dir(child);
                }
            }
            Some(Action::Copy(indices)) => {
                let entries: Vec<FtpEntry> = indices
                    .iter()
                    .filter_map(|&i| self.ftp.entries.get(i).cloned())
                    .collect();
                self.queue_download(entries, &path);
            }
            None => {}
        }
    }
}

impl FastFatApp {
    /// Reports a finished FTP transfer.
    pub(super) fn poll_transfers(&mut self, ctx: &egui::Context) {
        let Some(result) = self.transfers.poll(ctx) else {
            return;
        };
        self.status = match result {
            Ok(stats) => {
                let mut text = tf(
                    "job.copied",
                    &[
                        ("files", &stats.files),
                        ("size", &format_size(stats.bytes)),
                        (
                            "skipped",
                            &if stats.skipped > 0 {
                                tf("job.links_skipped", &[("links", &stats.skipped)])
                            } else {
                                String::new()
                            },
                        ),
                    ],
                );
                if stats.existing > 0 {
                    text.push_str(&tf(
                        "transfer.skipped_existing",
                        &[("count", &stats.existing)],
                    ));
                }
                Status::Success(text)
            }
            Err(FtpError::Cancelled) => Status::Info(Cow::Borrowed(t("job.cancelled_copy"))),
            Err(err) => Status::Error(err.to_string()),
        };
        self.local.reload();
    }
}
