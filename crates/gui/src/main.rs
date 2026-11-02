use std::collections::HashMap;
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use eframe::egui;
use eframe::egui::{Color32, RichText};
use rust_dl_core::clipboard::spawn_clipboard_watcher;
use rust_dl_core::downloader::{DownloadEngine, DownloadProgress as MediaDownloadProgress};
use rust_dl_core::queue::{JobId, JobStatus, QueueEvent, QueueJob, QueueManager};
use rust_dl_core::storage::{current_epoch_secs, AppConfig, HistoryEntry, HistoryStore};
use rust_dl_core::{
    BrowserKind, CookieSource, MediaManifest, Resolver, SidecarManager, StreamDelivery,
};
use tokio::runtime::{Handle, Runtime};
use tokio::sync::{mpsc, watch};

#[derive(Clone, Copy, PartialEq, Eq)]
enum QualityMode {
    Best,
    Max1080,
    Max720,
    AudioOnly,
    Ask,
}

impl QualityMode {
    fn label(self) -> &'static str {
        match self {
            Self::Best => "Best Video + Audio (Auto)",
            Self::Max1080 => "1080p Max",
            Self::Max720 => "720p Max",
            Self::AudioOnly => "Audio Only (M4A)",
            Self::Ask => "Ask Format Before Download",
        }
    }
}

struct RuntimeBundle {
    handle: Handle,
    queue: QueueManager,
    sidecar: SidecarManager,
    config: AppConfig,
    history: HistoryStore,
}

struct RustDlApp {
    handle: Handle,
    queue: QueueManager,
    sidecar: SidecarManager,
    input: String,
    quality: QualityMode,
    auto_capture: bool,
    allow_playlists: bool,
    config: AppConfig,
    history: HistoryStore,
    show_history: bool,
    jobs: HashMap<JobId, QueueJob>,
    selected_formats: HashMap<JobId, String>,
    event_receiver: std_mpsc::Receiver<QueueEvent>,
    clipboard_receiver: std_mpsc::Receiver<String>,
    clipboard_enabled: watch::Sender<bool>,
    status: Arc<Mutex<String>>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bundle = start_runtime()?;
    let app = RustDlApp::new(bundle);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([920.0, 720.0]),
        ..Default::default()
    };
    eframe::run_native(
        "rust-dl",
        options,
        Box::new(move |_creation_context| Ok(Box::new(app))),
    )?;
    Ok(())
}

fn start_runtime() -> Result<RuntimeBundle, Box<dyn std::error::Error>> {
    let (sender, receiver) = std_mpsc::sync_channel(1);
    thread::spawn(move || {
        let runtime = match Runtime::new() {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = sender.send(Err(error.to_string()));
                return;
            }
        };
        let setup = runtime.block_on(async {
            let sidecar = SidecarManager::new().map_err(|error| error.to_string())?;
            let config = AppConfig::load_default().await.unwrap_or_default();
            let history = HistoryStore::load_default().await.unwrap_or_default();
            let resolver = Resolver::new(sidecar.clone())
                .map_err(|error| error.to_string())?
                .with_max_playlist_items(config.max_playlist_items);
            let engine = DownloadEngine::new(sidecar.clone()).map_err(|error| error.to_string())?;
            let queue = QueueManager::new(
                resolver,
                engine,
                config.download_dir.clone(),
                config.max_concurrent_downloads,
            );
            queue.set_cookie_source(config.cookie_source.clone()).await;
            Ok::<RuntimeBundle, String>(RuntimeBundle {
                handle: runtime.handle().clone(),
                queue,
                sidecar,
                config,
                history,
            })
        });
        if sender.send(setup).is_ok() {
            runtime.block_on(std::future::pending::<()>())
        }
    });
    let result = receiver.recv().map_err(|error| error.to_string())?;
    result.map_err(|error| error.into())
}

impl RustDlApp {
    fn new(bundle: RuntimeBundle) -> Self {
        let (event_sender, event_receiver) = std_mpsc::channel();
        let mut queue_events = bundle.queue.subscribe();
        let event_bridge = event_sender;
        bundle.handle.spawn(async move {
            loop {
                match queue_events.recv().await {
                    Ok(event) => {
                        if event_bridge.send(event).is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        let (clipboard_enabled, enabled_receiver) = watch::channel(bundle.config.auto_clipboard);
        let (clipboard_sender, clipboard_receiver) = std_mpsc::channel();
        let clipboard_bridge = clipboard_sender;
        bundle.handle.spawn(async move {
            let mut clipboard_urls = spawn_clipboard_watcher(enabled_receiver);
            while let Some(url) = clipboard_urls.recv().await {
                if clipboard_bridge.send(url).is_err() {
                    break;
                }
            }
        });

        let status = Arc::new(Mutex::new("Checking media tools...".to_owned()));
        let bootstrap_status = Arc::clone(&status);
        let sidecar = bundle.sidecar.clone();
        bundle.handle.spawn(async move {
            set_status(&bootstrap_status, "Preparing media tools...");
            let (progress_sender, progress_receiver) = mpsc::channel(1);
            drop(progress_receiver);
            let result = sidecar.ensure_binaries(progress_sender).await;
            match result {
                Ok(()) => set_status(&bootstrap_status, "Media tools ready"),
                Err(error) => set_status(&bootstrap_status, &format!("Sidecar error: {error}")),
            }
        });

        Self {
            handle: bundle.handle,
            queue: bundle.queue,
            sidecar: bundle.sidecar,
            input: String::new(),
            quality: quality_from_preset(&bundle.config.quality_preset),
            auto_capture: bundle.config.auto_clipboard,
            allow_playlists: bundle.config.allow_playlists,
            config: bundle.config,
            history: bundle.history,
            show_history: false,
            jobs: HashMap::new(),
            selected_formats: HashMap::new(),
            event_receiver,
            clipboard_receiver,
            clipboard_enabled,
            status,
        }
    }

    fn enqueue_input(&self, input: String) {
        let queue = self.queue.clone();
        let quality = self.quality;
        let allow_playlists = self.allow_playlists;
        let status = Arc::clone(&self.status);
        self.handle.spawn(async move {
            let result = match quality {
                QualityMode::Best => {
                    queue
                        .enqueue_batch_auto(input, None, false, allow_playlists)
                        .await
                }
                QualityMode::Max1080 => {
                    queue
                        .enqueue_batch_auto(input, Some(1080), false, allow_playlists)
                        .await
                }
                QualityMode::Max720 => {
                    queue
                        .enqueue_batch_auto(input, Some(720), false, allow_playlists)
                        .await
                }
                QualityMode::AudioOnly => {
                    queue
                        .enqueue_batch_auto(input, None, true, allow_playlists)
                        .await
                }
                QualityMode::Ask => {
                    queue
                        .enqueue_batch_for_selection(input, allow_playlists)
                        .await
                }
            };
            if let Err(error) = result {
                set_status(&status, &error.to_string());
            }
        });
    }

    fn paste_and_download(&mut self) {
        match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text()) {
            Ok(text) => {
                self.input = text.trim().to_owned();
                if !self.input.is_empty() {
                    self.enqueue_input(self.input.clone());
                }
            }
            Err(error) => set_status(&self.status, &format!("Clipboard unavailable: {error}")),
        }
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.event_receiver.try_recv() {
            match event {
                QueueEvent::Updated(job) => {
                    let job = *job;
                    if let JobStatus::Completed { output_path } = &job.status {
                        self.record_history(&job, output_path);
                    }
                    if let JobStatus::Ready { manifest } = &job.status {
                        if let Some(first) = manifest.streams.first() {
                            self.selected_formats
                                .entry(job.id)
                                .or_insert_with(|| first.format_id.clone());
                        }
                    }
                    self.jobs.insert(job.id, job);
                }
                QueueEvent::Removed(id) => {
                    self.jobs.remove(&id);
                    self.selected_formats.remove(&id);
                }
            }
        }
        while let Ok(url) = self.clipboard_receiver.try_recv() {
            if self.auto_capture {
                self.enqueue_input(url);
            }
        }
    }

    fn record_history(&mut self, job: &QueueJob, output_path: &std::path::Path) {
        self.history.record(HistoryEntry {
            title: job.title.clone().unwrap_or_else(|| job.url.clone()),
            url: job.url.clone(),
            platform: job.platform,
            output_path: output_path.to_path_buf(),
            filesize_bytes: None,
            completed_at_epoch_secs: current_epoch_secs(),
        });
        let history = self.history.clone();
        self.handle.spawn(async move {
            let _ = history.save_default().await;
        });
    }

    fn save_config(&self) {
        let config = self.config.clone();
        self.handle.spawn(async move {
            let _ = config.save_default().await;
        });
    }

    fn apply_cookie_source(&mut self, source: CookieSource) {
        if self.config.cookie_source == source {
            return;
        }
        self.config.cookie_source = source.clone();
        let queue = self.queue.clone();
        self.handle.spawn(async move {
            queue.set_cookie_source(source).await;
        });
        self.save_config();
    }

    fn apply_quality(&mut self, quality: QualityMode) {
        if self.quality == quality {
            return;
        }
        self.quality = quality;
        self.config.quality_preset = quality_preset(quality).to_owned();
        self.save_config();
    }

    fn apply_auto_capture(&mut self, enabled: bool) {
        if self.auto_capture == enabled {
            return;
        }
        self.auto_capture = enabled;
        self.config.auto_clipboard = enabled;
        let _ = self.clipboard_enabled.send(enabled);
        self.save_config();
    }

    fn apply_allow_playlists(&mut self, enabled: bool) {
        if self.allow_playlists == enabled {
            return;
        }
        self.allow_playlists = enabled;
        self.config.allow_playlists = enabled;
        self.save_config();
    }

    fn update_extractors(&self) {
        let sidecar = self.sidecar.clone();
        let status = Arc::clone(&self.status);
        self.handle.spawn(async move {
            set_status(&status, "Updating yt-dlp...");
            match sidecar.update_ytdlp().await {
                Ok(report) => {
                    let state = if report.updated {
                        "Updated"
                    } else {
                        "Up to date"
                    };
                    set_status(
                        &status,
                        &format!("yt-dlp {} ({state})", report.current_version),
                    );
                }
                Err(error) => set_status(&status, &format!("yt-dlp update failed: {error}")),
            }
        });
    }

    fn handle_dropped_input(&mut self, context: &egui::Context) {
        let dropped = context.input(|input| input.raw.dropped_files.clone());
        for file in dropped {
            if file.name.starts_with("http://") || file.name.starts_with("https://") {
                self.enqueue_input(file.name);
            } else if let Some(path) = file.path {
                if path.to_string_lossy().starts_with("http://")
                    || path.to_string_lossy().starts_with("https://")
                {
                    self.enqueue_input(path.to_string_lossy().into_owned());
                    continue;
                }
                match std::fs::read_to_string(&path) {
                    Ok(contents) => self.enqueue_input(contents),
                    Err(error) => set_status(
                        &self.status,
                        &format!("Could not read dropped file: {error}"),
                    ),
                }
            } else if let Some(bytes) = file.bytes {
                match String::from_utf8(bytes.to_vec()) {
                    Ok(contents) => self.enqueue_input(contents),
                    Err(_) => set_status(&self.status, "Dropped data was not UTF-8 text"),
                }
            }
        }
        let pasted = context.input(|input| {
            input
                .raw
                .events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::Paste(text) => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        });
        for text in pasted {
            if !text.trim().is_empty() {
                self.enqueue_input(text);
            }
        }
    }
}

impl eframe::App for RustDlApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
        self.handle_dropped_input(context);
        let mut input_focused = false;
        egui::TopBottomPanel::top("top_bar").show(context, |ui| {
            ui.horizontal(|ui| {
                ui.heading("rust-dl");
                ui.separator();
                ui.label(status_text(&self.status));
            });
            ui.horizontal(|ui| {
                let input = ui.add(
                    egui::TextEdit::singleline(&mut self.input)
                        .hint_text("Paste a media URL")
                        .desired_width(500.0),
                );
                input_focused = input.has_focus();
                if ui.button("Paste & Download").clicked() {
                    self.paste_and_download();
                }
            });
            ui.horizontal(|ui| {
                let mut quality = self.quality;
                egui::ComboBox::from_label("Quality")
                    .selected_text(quality.label())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut quality,
                            QualityMode::Best,
                            QualityMode::Best.label(),
                        );
                        ui.selectable_value(
                            &mut quality,
                            QualityMode::Max1080,
                            QualityMode::Max1080.label(),
                        );
                        ui.selectable_value(
                            &mut quality,
                            QualityMode::Max720,
                            QualityMode::Max720.label(),
                        );
                        ui.selectable_value(
                            &mut quality,
                            QualityMode::AudioOnly,
                            QualityMode::AudioOnly.label(),
                        );
                        ui.selectable_value(
                            &mut quality,
                            QualityMode::Ask,
                            QualityMode::Ask.label(),
                        );
                    });
                if quality != self.quality {
                    self.apply_quality(quality);
                }
                let mut auto_capture = self.auto_capture;
                if ui
                    .checkbox(&mut auto_capture, "Auto-Capture Clipboard Links")
                    .changed()
                {
                    self.apply_auto_capture(auto_capture);
                }
                let mut allow_playlists = self.allow_playlists;
                if ui
                    .checkbox(&mut allow_playlists, "Expand Playlists / Carousels")
                    .changed()
                {
                    self.apply_allow_playlists(allow_playlists);
                }
                if ui.button("Update Extractors (yt-dlp)").clicked() {
                    self.update_extractors();
                }
                if ui
                    .button(if self.show_history {
                        "Queue"
                    } else {
                        "History"
                    })
                    .clicked()
                {
                    self.show_history = !self.show_history;
                }
            });
            self.render_settings(ui);
        });

        let paste_shortcut = context.input(|input| {
            input.modifiers.mac_cmd && input.key_pressed(egui::Key::V) && !input_focused
        });
        if paste_shortcut {
            self.paste_and_download();
        }

        egui::CentralPanel::default().show(context, |ui| {
            if self.show_history {
                self.render_history(ui);
            } else if self.jobs.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label("Paste a link to start downloading");
                });
            } else {
                if self.jobs.len() > 1 {
                    ui.horizontal(|ui| {
                        if ui.button("Clear Completed").clicked() {
                            let queue = self.queue.clone();
                            self.handle.spawn(async move {
                                let _ = queue.clear_completed();
                            });
                        }
                        if ui.button("Cancel All").clicked() {
                            let queue = self.queue.clone();
                            self.handle.spawn(async move {
                                let _ = queue.cancel_all();
                            });
                        }
                    });
                }
                let mut jobs: Vec<QueueJob> = self.jobs.values().cloned().collect();
                jobs.sort_by_key(|job| job.id);
                for job in jobs {
                    self.render_job_card(ui, job);
                }
            }
        });
        context.request_repaint_after(Duration::from_millis(100));
    }
}

impl RustDlApp {
    fn render_settings(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let mut source = self.config.cookie_source.clone();
            egui::ComboBox::from_label("Cookies")
                .selected_text(cookie_source_label(&source))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut source, CookieSource::None, "Off");
                    for browser in [
                        BrowserKind::Safari,
                        BrowserKind::Chrome,
                        BrowserKind::Arc,
                        BrowserKind::Brave,
                        BrowserKind::Firefox,
                        BrowserKind::Edge,
                    ] {
                        ui.selectable_value(
                            &mut source,
                            CookieSource::Browser(browser),
                            browser.to_string(),
                        );
                    }
                    ui.selectable_value(
                        &mut source,
                        CookieSource::CookieFile(std::path::PathBuf::new()),
                        "Custom File...",
                    );
                });
            if let CookieSource::CookieFile(path) = &mut source {
                let mut path_text = path.to_string_lossy().into_owned();
                ui.add(egui::TextEdit::singleline(&mut path_text).hint_text("cookies.txt"));
                *path = std::path::PathBuf::from(path_text);
            }
            if source != self.config.cookie_source {
                self.apply_cookie_source(source);
            }

            let mut concurrency = self.config.max_concurrent_downloads;
            egui::ComboBox::from_label("Concurrent")
                .selected_text(concurrency.to_string())
                .show_ui(ui, |ui| {
                    for value in 1..=8 {
                        ui.selectable_value(&mut concurrency, value, value.to_string());
                    }
                });
            if concurrency != self.config.max_concurrent_downloads {
                self.config.max_concurrent_downloads = concurrency;
                let queue = self.queue.clone();
                self.handle.spawn(async move {
                    queue.set_max_concurrent_downloads(concurrency).await;
                });
                self.save_config();
            }
        });
        ui.horizontal(|ui| {
            ui.label(format!("Output: {}", self.config.download_dir.display()));
            if ui.button("Open Folder").clicked() {
                let _ = std::process::Command::new("open")
                    .arg(&self.config.download_dir)
                    .status();
            }
        });
    }

    fn render_history(&mut self, ui: &mut egui::Ui) {
        ui.heading("History");
        if ui.button("Clear History").clicked() {
            self.history.clear();
            let history = self.history.clone();
            self.handle.spawn(async move {
                let _ = history.save_default().await;
            });
        }
        for entry in self.history.entries.clone() {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.label(RichText::new(&entry.title).strong());
                ui.label(format!("{} · {}", entry.platform, entry.url));
                ui.horizontal(|ui| {
                    if ui.button("Reveal in Finder").clicked() {
                        let _ = std::process::Command::new("open")
                            .arg("-R")
                            .arg(&entry.output_path)
                            .status();
                    }
                    if ui.button("Re-download").clicked() {
                        self.enqueue_input(entry.url.clone());
                    }
                });
            });
        }
    }

    fn render_job_card(&mut self, ui: &mut egui::Ui, job: QueueJob) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(platform_label(job.platform))
                        .strong()
                        .color(Color32::LIGHT_BLUE),
                );
                ui.label(job.title.as_deref().unwrap_or(&job.url));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("#{}", job.id));
                });
            });
            match &job.status {
                JobStatus::Queued => {
                    ui.label("Queued");
                }
                JobStatus::Resolving => {
                    ui.label("Resolving metadata...");
                }
                JobStatus::Ready { manifest } => {
                    self.render_ready(ui, &job, manifest);
                }
                JobStatus::Downloading { progress } => {
                    render_progress(ui, progress);
                    if ui.button("Cancel").clicked() {
                        let queue = self.queue.clone();
                        let id = job.id;
                        self.handle.spawn(async move {
                            let _ = queue.cancel_job(id);
                        });
                    }
                }
                JobStatus::Completed { output_path } => {
                    ui.label("Completed");
                    if ui.button("Reveal in Finder").clicked() {
                        let _ = std::process::Command::new("open")
                            .arg("-R")
                            .arg(output_path)
                            .status();
                    }
                }
                JobStatus::Failed { error } => {
                    ui.colored_label(Color32::LIGHT_RED, error);
                    if error
                        .to_ascii_lowercase()
                        .contains("authentication required")
                    {
                        ui.colored_label(
                            Color32::YELLOW,
                            "Auth required — select a Browser Cookie source above and click Retry",
                        );
                    }
                    if ui.button("Retry").clicked() {
                        let queue = self.queue.clone();
                        let id = job.id;
                        self.handle.spawn(async move {
                            let _ = queue.retry_job(id);
                        });
                    }
                }
                JobStatus::Cancelled => {
                    ui.label("Cancelled");
                    if ui.button("Retry").clicked() {
                        let queue = self.queue.clone();
                        let id = job.id;
                        self.handle.spawn(async move {
                            let _ = queue.retry_job(id);
                        });
                    }
                }
            }
            if ui.button("Remove").clicked() {
                let queue = self.queue.clone();
                let id = job.id;
                self.handle.spawn(async move {
                    let _ = queue.remove_job(id);
                });
            }
        });
        ui.add_space(8.0);
    }

    fn render_ready(&mut self, ui: &mut egui::Ui, job: &QueueJob, manifest: &MediaManifest) {
        let selected = self.selected_formats.entry(job.id).or_insert_with(|| {
            manifest
                .streams
                .first()
                .map(|stream| stream.format_id.clone())
                .unwrap_or_default()
        });
        egui::ComboBox::from_id_salt(("format", job.id))
            .selected_text(selected.clone())
            .show_ui(ui, |ui| {
                for stream in &manifest.streams {
                    let label = format!(
                        "{} · {} · {}",
                        stream.format_id,
                        stream.resolution.as_deref().unwrap_or("audio"),
                        delivery_label(&stream.delivery)
                    );
                    ui.selectable_value(selected, stream.format_id.clone(), label);
                }
            });
        if ui.button("Download").clicked() {
            let queue = self.queue.clone();
            let id = job.id;
            let format_id = selected.clone();
            self.handle.spawn(async move {
                let _ = queue.start_job(id, Some(format_id));
            });
        }
    }
}

fn render_progress(ui: &mut egui::Ui, progress: &MediaDownloadProgress) {
    let fraction = progress
        .total_bytes
        .filter(|total| *total > 0)
        .map(|total| progress.downloaded_bytes as f32 / total as f32)
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    ui.add(egui::ProgressBar::new(fraction).text(format!(
        "{:.0}% · {:.2} MB/s · ETA {}",
        fraction * 100.0,
        progress.speed_bytes_per_sec / 1_048_576.0,
        progress
            .eta_secs
            .map(|seconds| format!("{seconds}s"))
            .unwrap_or_else(|| "--".to_owned())
    )));
}

fn platform_label(platform: rust_dl_core::Platform) -> &'static str {
    match platform {
        rust_dl_core::Platform::YouTube => "YouTube",
        rust_dl_core::Platform::Instagram => "Instagram",
        rust_dl_core::Platform::Facebook => "Facebook",
        rust_dl_core::Platform::Pinterest => "Pinterest",
        rust_dl_core::Platform::Generic => "Direct",
    }
}

fn delivery_label(delivery: &StreamDelivery) -> &'static str {
    match delivery {
        StreamDelivery::Direct { .. } => "Direct",
        StreamDelivery::SplitDash { .. } => "DASH",
        StreamDelivery::Hls { .. } => "HLS",
    }
}

fn quality_from_preset(preset: &str) -> QualityMode {
    match preset {
        "1080p" => QualityMode::Max1080,
        "720p" => QualityMode::Max720,
        "audio" => QualityMode::AudioOnly,
        "ask" => QualityMode::Ask,
        _ => QualityMode::Best,
    }
}

fn quality_preset(quality: QualityMode) -> &'static str {
    match quality {
        QualityMode::Best => "best",
        QualityMode::Max1080 => "1080p",
        QualityMode::Max720 => "720p",
        QualityMode::AudioOnly => "audio",
        QualityMode::Ask => "ask",
    }
}

fn cookie_source_label(source: &CookieSource) -> String {
    match source {
        CookieSource::None => "Off".to_owned(),
        CookieSource::Browser(browser) => browser.to_string(),
        CookieSource::CookieFile(path) if path.as_os_str().is_empty() => {
            "Custom File...".to_owned()
        }
        CookieSource::CookieFile(path) => format!("File: {}", path.display()),
    }
}

fn set_status(status: &Arc<Mutex<String>>, value: &str) {
    match status.lock() {
        Ok(mut current) => *current = value.to_owned(),
        Err(poisoned) => *poisoned.into_inner() = value.to_owned(),
    }
}

fn status_text(status: &Arc<Mutex<String>>) -> String {
    match status.lock() {
        Ok(current) => current.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}
