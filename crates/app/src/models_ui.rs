//! Downloading what the AI tools need, from inside the window.
//!
//! The program ships without ONNX Runtime and the models. When a tool that
//! needs them is turned on and they are missing, a dialog says what is
//! needed (size, licence, where it goes) and offers the download; the
//! "AI models" window (Tools menu) shows every pack and downloads any of them.
//! Downloads run on their own thread with a progress bar and can be stopped.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use ai_models::{Feature, Pack, Progress};
use eframe::egui::{self, Button, RichText};

use crate::dialog::{Choice, dialog, dialog_buttons};
use crate::theme::{FONT, SPACE, Tokens};

/// What a download thread shares with the window.
#[derive(Debug, Default)]
struct Shared {
    progress: Option<Progress>,
    done: Option<Result<(), String>>,
}

/// A download in progress.
#[derive(Debug)]
struct Job {
    shared: Arc<Mutex<Shared>>,
    cancel: Arc<AtomicBool>,
}

/// The download dialog and the "AI models" window.
#[derive(Debug)]
pub struct ModelsUi {
    dir: PathBuf,
    /// The feature whose files the dialog offers to download.
    prompt: Option<Feature>,
    /// Features the user answered "Not now" for, in this session.
    declined: BTreeSet<Feature>,
    manager_open: bool,
    job: Option<Job>,
    /// The outcome of the last download, shown until the dialog is closed.
    outcome: Option<Result<(), String>>,
}

impl Default for ModelsUi {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelsUi {
    /// Nothing open, the models folder of this machine.
    #[must_use]
    pub fn new() -> Self {
        Self {
            dir: ai_models::models_dir(),
            prompt: None,
            declined: BTreeSet::new(),
            manager_open: false,
            job: None,
            outcome: None,
        }
    }

    /// The packs `feature` needs that are not installed.
    fn missing_for(&self, feature: Feature) -> Vec<Pack> {
        ai_models::missing(&ai_models::needs(feature), &self.dir)
    }

    /// `true` when what `feature` needs is installed; otherwise offers the
    /// download (once per session, unless the user asks again from the menu).
    pub fn require(&mut self, feature: Feature) -> bool {
        if self.missing_for(feature).is_empty() {
            return true;
        }
        if self.prompt.is_none() && self.job.is_none() && !self.declined.contains(&feature) {
            self.prompt = Some(feature);
            self.outcome = None;
        }
        false
    }

    /// Opens the "AI models" window.
    pub fn open_manager(&mut self) {
        self.manager_open = true;
        self.outcome = None;
    }

    /// `true` while the download dialog is up (the caller then skips its own error message).
    #[must_use]
    pub fn is_prompting(&self) -> bool {
        self.prompt.is_some()
    }

    fn start(&mut self, packs: Vec<Pack>, ctx: &egui::Context) {
        if self.job.is_some() || packs.is_empty() {
            return;
        }
        let shared = Arc::new(Mutex::new(Shared::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let (s, c, dir, repaint) = (Arc::clone(&shared), Arc::clone(&cancel), self.dir.clone(), ctx.clone());
        let spawned = std::thread::Builder::new().name("model-download".to_owned()).spawn(move || {
            let result = ai_models::download(
                &packs,
                &dir,
                &mut |p| {
                    s.lock().unwrap_or_else(PoisonError::into_inner).progress = Some(p.clone());
                    repaint.request_repaint();
                },
                &c,
            );
            s.lock().unwrap_or_else(PoisonError::into_inner).done = Some(result.map_err(|e| e.to_string()));
            repaint.request_repaint();
        });
        match spawned {
            Ok(_) => {
                self.outcome = None;
                self.job = Some(Job { shared, cancel });
            }
            Err(e) => self.outcome = Some(Err(format!("cannot start the download: {e}"))),
        }
    }

    /// Draws what is open. Returns `true` once when a download has finished
    /// successfully, so the photo can be rendered again with the new tool.
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        let mut installed = false;
        if let Some(job) = &self.job {
            let done = job.shared.lock().unwrap_or_else(PoisonError::into_inner).done.take();
            if let Some(result) = done {
                installed = result.is_ok();
                self.outcome = Some(result);
                self.job = None;
            }
        }
        if let Some(feature) = self.prompt {
            self.prompt_dialog(ctx, feature);
        } else if self.manager_open {
            self.manager(ctx);
        }
        if self.job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        installed
    }

    /// "AI denoise needs a download": what, how big, where; Download / Not now.
    fn prompt_dialog(&mut self, ctx: &egui::Context, feature: Feature) {
        let t = Tokens::of(ctx.theme());
        let missing = self.missing_for(feature);
        let (mut close, mut download, mut stop, mut decline) = (false, false, false, false);
        let this = &*self;
        dialog(ctx, "rne-model-prompt", |ui| {
            ui.label(RichText::new(format!("{} needs a download", feature.label())).size(FONT[4]).strong());
            ui.add_space(SPACE[1]);
            ui.label(
                "The AI models are not part of the program. They are downloaded once, \
                 then this tool works offline.",
            );
            ui.add_space(SPACE[2]);
            if missing.is_empty() && this.job.is_none() {
                ui.label(RichText::new("Everything needed is installed.").color(t.success));
            }
            for p in &missing {
                pack_line(ui, &t, p);
            }
            ui.add_space(SPACE[1]);
            ui.label(RichText::new(format!("Into {}", this.dir.display())).size(FONT[0]).color(t.text_weak));
            ui.add_space(SPACE[2]);
            this.job_status(ui, &t);
            ui.add_space(SPACE[2]);
            let busy = this.job.is_some();
            let (action, cancel) = if busy {
                (None, "Stop")
            } else if missing.is_empty() {
                (None, "Close")
            } else if matches!(this.outcome, Some(Err(_))) {
                (Some("Try again"), "Not now")
            } else {
                (Some("Download"), "Not now")
            };
            match dialog_buttons(ui, &t, action, cancel, |_| {}) {
                Some(Choice::Action) => download = true,
                Some(Choice::Cancel) if busy => stop = true,
                Some(Choice::Cancel) => {
                    decline = !missing.is_empty();
                    close = true;
                }
                None => {}
            }
        });
        if stop {
            self.cancel();
        }
        if decline {
            self.declined.insert(feature);
        }
        if download {
            self.start(missing, ctx);
        }
        if close {
            self.prompt = None;
            self.outcome = None;
        }
    }

    /// Every pack: installed or not, size, licence, a download button.
    fn manager(&mut self, ctx: &egui::Context) {
        let t = Tokens::of(ctx.theme());
        let packs = ai_models::catalog();
        let mut start: Option<Vec<Pack>> = None;
        let (mut close, mut stop) = (false, false);
        let mut folder_error = None;
        let busy = self.job.is_some();
        let this = &*self;
        dialog(ctx, "rne-model-manager", |ui| {
            ui.label(RichText::new("AI models").size(FONT[4]).strong());
            ui.add_space(SPACE[1]);
            ui.label("Downloaded on demand, kept on this computer. Optional packs only make a tool faster.");
            ui.add_space(SPACE[2]);
            let mut all_missing = Vec::new();
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                for p in &packs {
                    let installed = ai_models::missing_files(p, &this.dir).is_empty();
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            pack_line(ui, &t, p);
                            ui.label(RichText::new(p.purpose).size(FONT[0]).color(t.text_weak));
                            ui.hyperlink_to(RichText::new(source_text(p.source)).size(FONT[0]), p.source)
                                .on_hover_text(p.source);
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if installed {
                                ui.label(RichText::new("Installed").color(t.success));
                            } else if ui.add_enabled(!busy, Button::new("Download")).clicked() {
                                start = Some(vec![p.clone()]);
                            }
                        });
                    });
                    ui.add_space(SPACE[1]);
                    if !installed && !p.optional {
                        all_missing.push(p.clone());
                    }
                }
            });
            ui.add_space(SPACE[1]);
            ui.label(RichText::new(format!("Folder: {}", this.dir.display())).size(FONT[0]).color(t.text_weak));
            ui.add_space(SPACE[2]);
            this.job_status(ui, &t);
            ui.add_space(SPACE[2]);
            // Every pack a tool needs; optional ones are left out.
            let action = (!busy && !all_missing.is_empty()).then_some("Download everything needed");
            let cancel = if busy { "Stop" } else { "Close" };
            let choice = dialog_buttons(ui, &t, action, cancel, |ui| {
                if ui.button("Open folder").clicked() {
                    let _ = std::fs::create_dir_all(&this.dir);
                    let launch = platform::reveal_in_file_manager(platform::Os::current(), &this.dir);
                    if let Err(e) = std::process::Command::new(launch.program).args(&launch.args).spawn() {
                        folder_error = Some(format!("cannot open the folder: {e}"));
                    }
                }
            });
            match choice {
                Some(Choice::Action) => start = Some(std::mem::take(&mut all_missing)),
                Some(Choice::Cancel) if busy => stop = true,
                Some(Choice::Cancel) => close = true,
                None => {}
            }
        });
        if stop {
            self.cancel();
        }
        if let Some(message) = folder_error {
            self.outcome = Some(Err(message));
        }
        if let Some(packs) = start {
            self.start(packs, ctx);
        }
        if close {
            self.manager_open = false;
            self.outcome = None;
        }
    }

    /// Progress of the running download, or how the last one ended.
    fn job_status(&self, ui: &mut egui::Ui, t: &Tokens) {
        if let Some(job) = &self.job {
            let progress = job.shared.lock().unwrap_or_else(PoisonError::into_inner).progress.clone();
            match progress {
                Some(p) => {
                    let size = match p.total {
                        Some(total) => format!("{} of {}", ai_models::megabytes(p.done), ai_models::megabytes(total)),
                        None => ai_models::megabytes(p.done),
                    };
                    ui.label(format!("{} \u{b7} file {} of {} \u{b7} {size}", p.pack, p.index, p.count));
                    match p.fraction() {
                        Some(f) => ui.add(egui::ProgressBar::new(f).show_percentage()),
                        None => ui.add(egui::ProgressBar::new(0.0).animate(true)),
                    };
                }
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Starting the download\u{2026}");
                    });
                }
            }
        } else {
            match &self.outcome {
                Some(Ok(())) => {
                    ui.label(RichText::new("Download complete.").color(t.success));
                }
                Some(Err(message)) => {
                    ui.label(RichText::new(message).color(t.error));
                }
                None => {}
            }
        }
    }

    fn cancel(&self) {
        if let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// "From github.com/cszn/SCUNet": where a pack comes from, without the scheme.
fn source_text(url: &str) -> String {
    let bare = url.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/');
    format!("From {bare}")
}

/// "SCUNet (AI denoise) · 72 MB · Apache-2.0".
fn pack_line(ui: &mut egui::Ui, t: &Tokens, p: &Pack) {
    let optional = if p.optional { " \u{b7} optional" } else { "" };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(p.name).strong().color(t.text_strong));
        ui.label(RichText::new(format!("{} \u{b7} {}{optional}", p.size_text(), p.license)).color(t.text_weak));
    });
}
