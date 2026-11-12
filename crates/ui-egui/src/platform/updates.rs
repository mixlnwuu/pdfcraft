//! Help ▸ Check for updates (issue #28): ask for the latest release and offer its download page.
//!
//! The desktop app supplies how to ask ([`PdfCraftApp::update_source`]), so this crate has no
//! network code; without a source (the web build, tests) the command opens the releases page.
//! Thai edition: the desktop app also asks at the first start of a day (Preferences can turn that
//! off, and the prompt can put a version aside), and a
//! newer release gets a prompt. Where the app can install it ([`UpdateInstall`]: Windows, not a
//! portable copy), Update and restart downloads the release's MSI, checks its SHA-256 against
//! GitHub's, and runs it once PdfCraft has closed; elsewhere the prompt opens the release's page.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use egui::{Align, Layout};

use crate::{PdfCraftApp, theme, widgets};

/// Where every release of the Thai edition is listed (Thai edition: its own repository, not
/// upstream's, so the check offers the edition's next release).
/// Bridge release (0.20.0-th.2): MaewPDF's releases, where this edition's users move to.
pub const RELEASES_PAGE: &str = "https://github.com/mixlnwuu/maewpdf/releases";

/// The latest published release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// Its version tag, such as `v0.2.0`.
    pub version: String,
    /// Its page on [`RELEASES_PAGE`], where the downloads are.
    pub url: String,
    /// Its Windows installer, when it lists one with a checksum.
    pub installer: Option<Installer>,
}

/// A release's Windows installer, as GitHub lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installer {
    pub url: String,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
}

/// Downloads and checks an installer (blocking; it runs on its own thread), telling how far it
/// is, 0 to 1, and stopping when the flag is set. Returns the file.
pub type UpdateDownload = Arc<dyn Fn(&Installer, &(dyn Fn(f32) + Sync), &AtomicBool) -> Result<PathBuf, String> + Send + Sync>;

/// Starts an installer for when PdfCraft has closed, and PdfCraft again after it.
pub type UpdateRun = Arc<dyn Fn(&Path) -> Result<(), String> + Send + Sync>;

/// How the desktop app installs a release (none: the prompt opens the release's page).
#[derive(Clone)]
pub struct UpdateInstall {
    pub download: UpdateDownload,
    pub run: UpdateRun,
}

/// Asks for the latest release (blocking; it runs on its own thread).
pub type UpdateSource = Arc<dyn Fn() -> Result<Release, String> + Send + Sync>;

/// The version the update check compares with: this build's, or `PDFCRAFT_PRETEND_VERSION`, for
/// seeing the prompt on a build that is already the latest.
fn current_version() -> String {
    std::env::var("PDFCRAFT_PRETEND_VERSION")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| pdfcraft_engine::build_info::edition_version().to_owned())
}

/// Whether release `latest` (a tag such as `v0.2.0`) is newer than version `current` (`0.1.1`).
/// A Thai edition suffix `-th.N` counts after the numbers (`0.10.1-th.2` is newer than
/// `0.10.1-th.1`, which is newer than `0.10.1`); other pre-release and build suffixes are
/// ignored. A version that doesn't parse is never newer.
pub fn is_newer(latest: &str, current: &str) -> bool {
    matches!((parse(latest), parse(current)), (Some(l), Some(c)) if l > c)
}

fn parse(v: &str) -> Option<(u64, u64, u64, u64)> {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let (core, suffix) = match v.split_once(['-', '+']) {
        Some((core, suffix)) => (core, Some(suffix)),
        None => (v, None),
    };
    let mut parts = core.split('.');
    let mut next = |required: bool| match parts.next() {
        Some(p) => p.parse::<u64>().ok(),
        None if required => None,
        None => Some(0),
    };
    let (major, minor, patch) = (next(true)?, next(false)?, next(false)?);
    if parts.next().is_some() {
        return None;
    }
    // The Thai edition's release number (Thai edition); a `-th.` that isn't one is no version.
    let edition = match suffix.and_then(|s| s.strip_prefix("th.")) {
        Some(n) => n.split(['-', '+']).next()?.parse::<u64>().ok()?,
        None => 0,
    };
    Some((major, minor, patch, edition))
}

/// Where a check is.
#[derive(Default)]
pub(crate) enum Check {
    #[default]
    Idle,
    #[cfg(not(target_arch = "wasm32"))]
    Running(std::sync::mpsc::Receiver<Result<Release, String>>),
    Done(Result<Release, String>),
}

#[derive(Default)]
pub(crate) struct Updates {
    pub(crate) check: Check,
    /// The Updates dialog is showing.
    pub(crate) open: bool,
    /// Ask once at start; the desktop app turns it on, so tests never reach the network.
    start_check: bool,
    /// The check under way is the one at start: it says nothing unless there is a release.
    quiet: bool,
    /// A newer release offered.
    prompt: Option<Prompt>,
    /// The prompt's "Don't remind me about this version" box.
    skip_ticked: bool,
    /// The installer downloaded and checked, to run as PdfCraft closes.
    ready: Option<PathBuf>,
}

/// A newer release being offered.
struct Prompt {
    release: Release,
    state: PromptState,
}

enum PromptState {
    Offered,
    /// How far (a fraction, as the bits of an `f32`), what stops it, and its outcome.
    Downloading {
        progress: Arc<AtomicU32>,
        stop: Arc<AtomicBool>,
        done: std::sync::mpsc::Receiver<Result<PathBuf, String>>,
    },
    Failed(String),
}

impl PdfCraftApp {
    /// Whether the app asks for a newer release on its first frame (off until the desktop app
    /// turns it on), when Preferences allow it.
    pub fn set_startup_update_check(&mut self, on: bool) {
        self.updates.start_check = on;
    }

    /// Help ▸ Check for updates: ask for the latest release and show the outcome.
    pub fn check_for_updates(&mut self) {
        if self.update_source.is_none() {
            self.open_url(RELEASES_PAGE);
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.updates.open = true;
            self.updates.quiet = false;
            self.start_check();
        }
        #[cfg(target_arch = "wasm32")]
        self.open_url(RELEASES_PAGE);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn start_check(&mut self) {
        if matches!(self.updates.check, Check::Running(_)) {
            return;
        }
        let Some(source) = self.update_source.clone() else { return };
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            // The receiver may be gone (the app quit): nothing to report to then.
            let _ = tx.send(source());
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        });
        self.updates.check = Check::Running(rx);
    }

    /// Pick up a finished check (each frame), and start the one at start.
    pub(crate) fn poll_updates(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if std::mem::take(&mut self.updates.start_check) {
                self.check_update_took();
                let today = chrono::Local::now().format("%Y-%m-%d").to_string();
                // Bridge release: every start offers MaewPDF, whatever Preferences say.
                {
                    self.update_asked_on = today;
                    self.updates.quiet = true;
                    self.start_check();
                }
            }
            if let Check::Running(rx) = &self.updates.check {
                let result = match rx.try_recv() {
                    Ok(r) => r,
                    Err(std::sync::mpsc::TryRecvError::Empty) => return,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("the update check stopped unexpectedly".into()),
                };
                let current = current_version();
                let newer = result.as_ref().ok().filter(|r| is_newer(&r.version, &current));
                let quiet = std::mem::take(&mut self.updates.quiet);
                // A version put aside is said no more at start; asked for by hand, it still is.
                // Bridge release: MaewPDF is never put aside.
                let skipped = false;
                match newer.filter(|_| !skipped) {
                    Some(r) if quiet || (r.installer.is_some() && self.update_install.is_some()) => {
                        self.updates.prompt = Some(Prompt { release: r.clone(), state: PromptState::Offered });
                        self.updates.open = false;
                        self.updates.check = Check::Idle;
                    }
                    // Nobody asked: a check at start that finds nothing says nothing.
                    _ if quiet => self.updates.check = Check::Idle,
                    _ => self.updates.check = Check::Done(result),
                }
            }
        }
    }

    /// Download `release`'s installer on its own thread.
    fn start_download(&mut self, ctx: &egui::Context) {
        let (Some(prompt), Some(install)) = (self.updates.prompt.as_mut(), self.update_install.clone()) else { return };
        let Some(installer) = prompt.release.installer.clone() else { return };
        let progress = Arc::new(AtomicU32::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, done) = std::sync::mpsc::channel();
        let (seen, stopped, ctx) = (progress.clone(), stop.clone(), ctx.clone());
        std::thread::spawn(move || {
            let report = |fraction: f32| {
                seen.store(fraction.to_bits(), Ordering::Relaxed);
                ctx.request_repaint();
            };
            // The receiver may be gone (the app quit): nothing to report to then.
            let _ = tx.send((install.download)(&installer, &report, &stopped));
            ctx.request_repaint();
        });
        prompt.state = PromptState::Downloading { progress, stop, done };
    }

    /// The first frame after an update ran: this build has to be the version it installed.
    /// Anything else (the installer was refused or failed) is said, not left to be noticed.
    #[cfg(not(target_arch = "wasm32"))]
    fn check_update_took(&mut self) {
        let Some(wanted) = self.update_attempt.take() else { return };
        let current = pdfcraft_engine::build_info::edition_version();
        if is_newer(&wanted, current) {
            let message = crate::i18n::fmt(
                tl!("The update to PdfCraft {v} wasn't installed: you still have {c}. It can be downloaded from its release page."),
                &[("v", wanted.trim_start_matches(['v', 'V'])), ("c", current)],
            );
            self.notify(message);
        }
    }

    /// Start the downloaded installer, as PdfCraft closes.
    pub(crate) fn run_ready_update(&mut self) {
        if let (Some(path), Some(install)) = (self.updates.ready.take(), self.update_install.clone())
            && let Err(e) = (install.run)(&path)
        {
            log::warn!("couldn't start the update: {e}");
        }
    }
}

/// The prompt for a newer release, as Craft apps show theirs (Thai edition).
fn prompt(app: &mut PdfCraftApp, ctx: &egui::Context) {
    // A finished download: close, which runs it. Unsaved work asks first, and the update then
    // waits for PdfCraft to close.
    if let Some(Prompt { state: PromptState::Downloading { done, .. }, .. }) = app.updates.prompt.as_ref() {
        match done.try_recv() {
            Ok(Ok(path)) => {
                app.update_attempt = app.updates.prompt.take().map(|p| p.release.version);
                app.updates.ready = Some(path);
                if !app.request_quit(ctx) {
                    app.notify(tl!("The update will be installed when PdfCraft closes."));
                }
                return;
            }
            Ok(Err(e)) => {
                if let Some(p) = app.updates.prompt.as_mut() {
                    p.state = PromptState::Failed(e);
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                if let Some(p) = app.updates.prompt.as_mut() {
                    p.state = PromptState::Failed("the download stopped unexpectedly".into());
                }
            }
        }
    }
    let can_install = app.update_install.is_some();
    let Some(p) = app.updates.prompt.as_ref() else { return };
    let t = theme::Tokens::get(ctx);
    let current = current_version();
    let version = p.release.version.trim_start_matches(['v', 'V']).to_owned();
    let page = p.release.url.clone();
    let installable = can_install && p.release.installer.is_some();
    let (mut close, mut update, mut download, mut cancel) = (false, false, false, false);
    let skip_ticked = app.updates.skip_ticked;
    let tag = p.release.version.clone();
    let modal = crate::widgets::show_modal(ctx, "update-prompt", |ui| {
        ui.set_width(440.0);
        ui.horizontal(|ui| {
            crate::thai_edition_mark(ui, 56.0);
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("MaewPDF").font(theme::semibold(18.0)));
                ui.label(egui::RichText::new(format!("v{current} \u{2192} {version}")).monospace().color(t.text_muted));
            });
        });
        ui.add_space(10.0);
        // Bridge release: says where the edition went; the move is never put aside (no checkbox).
        ui.label(egui::RichText::new("PdfCraft ภาษาไทยเปลี่ยนชื่อเป็น MaewPDF แล้ว").strong());
        ui.add_space(4.0);
        ui.label(crate::i18n::fmt(
            "กด \"ย้ายไป MaewPDF\" เพื่อติดตั้ง MaewPDF {v} การตั้งค่าและไฟล์ล่าสุดจะตามไปด้วย และ PdfCraft ภาษาไทยรุ่นนี้จะถูกถอนออกให้ (PdfCraft ฉบับทางการ ถ้ามี จะไม่ถูกแตะ)",
            &[("v", &version)],
        ));
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);
        match &p.state {
            PromptState::Downloading { progress, .. } => {
                let fraction = f32::from_bits(progress.load(Ordering::Relaxed)).clamp(0.0, 1.0);
                ui.add(egui::ProgressBar::new(fraction).show_percentage().text(tl!("Downloading the update…")));
                ui.add_space(6.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    cancel = widgets::pill_button(ui, tl!("Cancel"), false).clicked();
                });
            }
            state => {
                if let PromptState::Failed(e) = state {
                    ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Couldn't update: {e}"), &[("e", e)])).color(t.text_muted));
                    ui.add_space(6.0);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let failed = matches!(state, PromptState::Failed(_));
                    if installable && !failed {
                        update = widgets::pill_button(ui, "ย้ายไป MaewPDF", true).clicked();
                    } else {
                        download = widgets::pill_button(ui, tl!("Download"), true).clicked();
                    }
                    close = widgets::pill_button(ui, tl!("Later"), false).clicked();
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.hyperlink_to(tl!("What's new"), &page);
                    });
                });
            }
        }
    });
    app.updates.skip_ticked = skip_ticked;
    let downloading = matches!(p.state, PromptState::Downloading { .. });
    if cancel || (downloading && modal.should_close()) {
        if let Some(Prompt { state: PromptState::Downloading { stop, .. }, .. }) = app.updates.prompt.as_ref() {
            stop.store(true, Ordering::Relaxed);
        }
        app.updates.prompt = None;
    } else if close || modal.should_close() {
        app.updates.prompt = None;
        if std::mem::take(&mut app.updates.skip_ticked) {
            app.update_skipped = Some(tag);
        }
    } else if update {
        app.start_download(ctx);
    } else if download {
        app.updates.prompt = None;
        app.open_url(&page);
    }
}

/// The Updates dialog.
pub(crate) fn dialog(app: &mut PdfCraftApp, ctx: &egui::Context) {
    prompt(app, ctx);
    if !app.updates.open {
        return;
    }
    let t = theme::Tokens::get(ctx);
    // The edition's own version, with its -th.N (Thai edition).
    let current = pdfcraft_engine::build_info::edition_version();
    let mut close = false;
    let mut download: Option<String> = None;
    let modal = crate::widgets::show_modal(ctx, "updates", |ui| {
        ui.set_width(420.0);
        ui.horizontal(|ui| {
            ui.add(crate::icons::image("cloud", 22.0, t.accent));
            ui.label(egui::RichText::new(tl!("Check for updates")).font(theme::semibold(16.0)));
        });
        ui.add_space(8.0);
        match &app.updates.check {
            Check::Idle => {
                ui.label(tl!("No check has run yet."));
            }
            #[cfg(not(target_arch = "wasm32"))]
            Check::Running(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tl!("Checking for a newer version…"));
                });
            }
            Check::Done(Ok(r)) if is_newer(&r.version, current) => {
                let version = r.version.trim_start_matches(['v', 'V']);
                ui.label(egui::RichText::new(crate::i18n::fmt(tl!("PdfCraft {v} is available."), &[("v", version)])).strong());
                ui.label(
                    egui::RichText::new(crate::i18n::fmt(
                        tl!("You have version {c}. Download the new version from its release page."),
                        &[("c", current)],
                    ))
                    .color(t.text_muted),
                );
                download = Some(r.url.clone());
            }
            Check::Done(Ok(_)) => {
                ui.label(crate::i18n::fmt(tl!("PdfCraft {c} is up to date."), &[("c", current)]));
            }
            Check::Done(Err(e)) => {
                ui.label(crate::i18n::fmt(tl!("Couldn't check for updates: {e}"), &[("e", &e.to_string())]));
                ui.label(
                    egui::RichText::new(crate::i18n::fmt(
                        tl!("You have version {c}. All releases are listed at {page}."),
                        &[("c", current), ("page", RELEASES_PAGE)],
                    ))
                    .color(t.text_muted),
                );
            }
        }
        ui.add_space(10.0);
        ui.label(
            egui::RichText::new(tl!("Asks GitHub for the latest release. Nothing is downloaded or installed automatically."))
                .color(t.text_muted)
                .small(),
        );
        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if let Some(url) = download.take() {
                let get = widgets::pill_button(ui, tl!("Download"), true).clicked();
                let later = widgets::pill_button(ui, tl!("Later"), false).clicked();
                close = get || later;
                download = get.then_some(url);
            } else if widgets::pill_button(ui, tl!("Close"), true).clicked() {
                close = true;
            }
        });
    });
    if modal.should_close() {
        close = true;
        download = None;
    }
    if close {
        app.updates.open = false;
        if let Some(url) = download {
            app.open_url(&url);
        }
    }
}
