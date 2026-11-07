//! The egui application: window setup, shared state and the frame loop.
//!
//! * [`job`]: worker threads, progress and results.
//! * [`format_tab`]: the FAT32 formatting page.
//! * [`clone_tab`]: the disk cloning page.
//! * [`ftp_tab`]: downloading from the Wii U over LAN (FTP).
//! * [`browser`]: the listing and drop zone shared by both.
//! * [`dialog`]: the confirmation dialog.

mod browser;
mod clone_tab;
mod dialog;
mod fonts;
mod format_tab;
mod ftp_tab;
mod i18n;
mod job;
mod transfer;

use std::borrow::Cow;

use eframe::egui;
use fastfat_device::{self as device, DiskInfo, DriveInfo};
use fastfat_fat32::VolumeLabel;

use i18n::t;
use job::Job;

/// Starts the native window and runs the event loop.
///
/// Raw disk access needs administrator rights: when they are missing, the
/// program asks for them through UAC and lets the elevated instance take
/// over. When the user declines, it goes on without them and shows a warning.
///
/// # Errors
///
/// Returns an error when the window or the graphics context cannot be
/// created.
pub fn run() -> eframe::Result {
    i18n::load_language();
    let elevated = device::is_elevated();
    if !elevated && device::relaunch_elevated().is_ok() {
        return Ok(());
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([760.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "FastFAT",
        options,
        Box::new(move |cc| {
            fonts::install_japanese_font(&cc.egui_ctx);
            Ok(Box::new(FastFatApp::new(elevated)))
        }),
    )
}

/// Message shown at the bottom of the window.
enum Status {
    /// No message.
    None,
    /// Neutral information.
    Info(Cow<'static, str>),
    /// The last operation succeeded.
    Success(String),
    /// The last operation failed.
    Error(String),
}

/// Page shown in the window.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    /// FAT32 formatting.
    Format,
    /// Whole-disk cloning.
    Clone,
    /// Downloading over LAN.
    Ftp,
}

/// Dialog waiting for confirmation.
#[derive(Clone, Copy)]
enum Confirm {
    /// Format with this validated label.
    Format(VolumeLabel),
    /// Clone the selected disks.
    Clone,
}

/// Application state.
struct FastFatApp {
    /// Whether the process has administrator rights.
    elevated: bool,
    /// Page shown in the window.
    tab: Tab,
    /// Drives that can be formatted.
    drives: Vec<DriveInfo>,
    /// Letter of the selected drive.
    selected: Option<char>,
    /// Whether fixed drives (HDDs) are listed.
    show_fixed: bool,
    /// Cluster size in bytes, `None` for automatic.
    cluster_size: Option<u32>,
    /// Text of the volume label input.
    label: String,
    /// Whether to skip zeroing the data region.
    quick: bool,
    /// Whether to re-initialise the whole disk as MBR (Wii U compatible).
    wii_u: bool,
    /// Physical disks for cloning.
    disks: Vec<DiskInfo>,
    /// Disk number of the clone source.
    clone_source: Option<u32>,
    /// Disk number of the clone destination.
    clone_destination: Option<u32>,
    /// Whether the clone gets a fresh MBR disk signature.
    new_signature: bool,
    /// Layout of the clone destination.
    clone_layout: clone_tab::CloneLayout,
    /// Size of the empty Wii U storage partition in GiB.
    storage_gib: u32,
    /// FTP page.
    ftp: ftp_tab::FtpState,
    /// Background FTP downloads.
    transfers: transfer::Transfers,
    /// PC pane shared by the Wii U pages.
    local: browser::LocalPane,
    /// Dialog waiting for confirmation, if any.
    confirm: Option<Confirm>,
    /// Running job, if any.
    job: Option<Job>,
    /// Status message.
    status: Status,
}

impl FastFatApp {
    /// Creates the application and lists the drives and disks.
    fn new(elevated: bool) -> Self {
        let mut app = Self {
            elevated,
            tab: Tab::Format,
            drives: Vec::new(),
            selected: None,
            show_fixed: false,
            cluster_size: None,
            label: String::new(),
            quick: true,
            wii_u: false,
            disks: Vec::new(),
            clone_source: None,
            clone_destination: None,
            new_signature: true,
            clone_layout: clone_tab::CloneLayout::WiiU,
            storage_gib: clone_tab::DEFAULT_STORAGE_GIB,
            ftp: ftp_tab::FtpState::default(),
            transfers: transfer::Transfers::default(),
            local: browser::LocalPane::default(),
            confirm: None,
            job: None,
            status: Status::None,
        };
        app.refresh();
        app
    }

    /// Re-enumerates drives and disks, keeping selections that still exist.
    fn refresh(&mut self) {
        self.drives = device::list_drives(self.show_fixed);
        if self.selected_drive().is_none() {
            self.selected = self.drives.first().map(|d| d.letter);
        }

        self.disks = device::list_disks();
        if find_disk(&self.disks, self.clone_source).is_none() {
            self.clone_source = self.disks.iter().find(|d| !d.is_system).map(|d| d.number);
        }
        if find_disk(&self.disks, self.clone_destination).is_none() {
            self.clone_destination = None;
        }
    }

    /// Returns the selected drive.
    fn selected_drive(&self) -> Option<&DriveInfo> {
        find_drive(&self.drives, self.selected)
    }

    /// Draws the status message.
    fn status_section(&self, ui: &mut egui::Ui) {
        match &self.status {
            Status::None => {}
            Status::Info(text) => {
                ui.label(text.as_ref());
            }
            Status::Success(text) => {
                ui.colored_label(egui::Color32::from_rgb(0x2E, 0xA0, 0x43), text.as_str());
            }
            Status::Error(text) => {
                ui.colored_label(ui.visuals().error_fg_color, text.as_str());
            }
        }
    }

    /// Keeps the window open while a job is running.
    fn guard_close(&mut self, ctx: &egui::Context) {
        let busy = self.job.is_some() || self.transfers.is_busy();
        if busy && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.status = Status::Info(Cow::Borrowed(t("app.busy_close")));
        }
    }
}

impl eframe::App for FastFatApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_job(ui.ctx());
        self.poll_transfers(ui.ctx());
        self.guard_close(ui.ctx());

        // Progress and messages live in a bottom panel whose top edge can be
        // dragged, so they never fall off the window.
        let show_bottom =
            self.transfers.is_busy() || self.job.is_some() || !matches!(self.status, Status::None);
        if show_bottom {
            egui::Panel::bottom("bottom")
                .resizable(true)
                .default_size(BOTTOM_HEIGHT)
                .size_range(BOTTOM_MIN_HEIGHT..=BOTTOM_MAX_HEIGHT)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            self.transfers.draw(ui);
                            self.progress_section(ui);
                            ui.add_space(4.0);
                            self.status_section(ui);
                        });
                });
        }

        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(t("app.title"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    language_selector(ui);
                });
            });
            if !self.elevated {
                ui.colored_label(ui.visuals().error_fg_color, t("app.not_admin"));
            }
            let busy = self.job.is_some();
            ui.add_enabled_ui(!busy, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tab, Tab::Format, t("tab.format"));
                    ui.selectable_value(&mut self.tab, Tab::Clone, t("tab.clone"));
                    ui.selectable_value(&mut self.tab, Tab::Ftp, t("tab.ftp"));
                });
                ui.separator();
                match self.tab {
                    Tab::Format => {
                        self.drive_section(ui);
                        ui.add_space(8.0);
                        self.options_section(ui);
                    }
                    Tab::Clone => self.clone_section(ui),
                    Tab::Ftp => self.ftp_section(ui),
                }
            });
        });

        self.confirm_dialog(ui.ctx());
    }
}

/// Initial height of the bottom panel (progress and messages).
const BOTTOM_HEIGHT: f32 = 170.0;

/// Smallest height the bottom panel can be dragged to.
const BOTTOM_MIN_HEIGHT: f32 = 40.0;

/// Largest height the bottom panel can be dragged to.
const BOTTOM_MAX_HEIGHT: f32 = 800.0;

/// Finds the drive with the letter `selected` in `drives`.
fn find_drive(drives: &[DriveInfo], selected: Option<char>) -> Option<&DriveInfo> {
    drives.iter().find(|d| Some(d.letter) == selected)
}

/// Finds the disk with the number `selected` in `disks`.
fn find_disk(disks: &[DiskInfo], selected: Option<u32>) -> Option<&DiskInfo> {
    disks.iter().find(|d| Some(d.number) == selected)
}

/// Draws the language selector.
fn language_selector(ui: &mut egui::Ui) {
    let current = i18n::language();
    egui::ComboBox::from_id_salt("language")
        .selected_text(current.native_name())
        .show_ui(ui, |ui| {
            for language in i18n::Language::ALL {
                if ui
                    .selectable_label(language == current, language.native_name())
                    .clicked()
                {
                    i18n::set_language(language);
                }
            }
        });
    ui.label("🌐");
}
