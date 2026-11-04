mod queue_view;
mod staging;
mod theme;

use std::collections::HashMap;
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use eframe::egui;
use eframe::egui::RichText;
use rust_dl_core::clipboard::spawn_clipboard_watcher;
use rust_dl_core::downloader::DownloadEngine;
use rust_dl_core::queue::{JobId, JobStatus, QueueEvent, QueueJob, QueueManager};
use rust_dl_core::storage::{current_epoch_secs, AppConfig, HistoryEntry, HistoryStore};
use rust_dl_core::{BrowserKind, CookieSource, Resolver, SidecarManager};
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
    resolver: Resolver,
    sidecar: SidecarManager,
    config: AppConfig,
    history: HistoryStore,
}

struct RustDlApp {
    handle: Handle,
    queue: QueueManager,
    resolver: Resolver,
    sidecar: SidecarManager,
    staging: staging::StagingState,
    staging_sender: std_mpsc::Sender<staging::StagingEvent>,
    staging_receiver: std_mpsc::Receiver<staging::StagingEvent>,
    input: String,
    quality: QualityMode,
    auto_capture: bool,
    allow_playlists: bool,
    config: AppConfig,
    history: HistoryStore,
    show_history: bool,
    jobs: HashMap<JobId, QueueJob>,
    #[allow(dead_code)]
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
        viewport: egui::ViewportBuilder::default()
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false)
            .with_titlebar_buttons_shown(true)
            .with_transparent(true)
            .with_inner_size([960.0, 720.0])
            .with_min_inner_size([760.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "rust-dl",
        options,
        Box::new(move |cc| {
            theme::setup_system_fonts(&cc.egui_ctx);
            theme::apply_macos_vibrancy(cc);
            theme::configure_visuals(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
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
                resolver.clone(),
                engine,
                config.download_dir.clone(),
                config.max_concurrent_downloads,
            );
            queue.set_cookie_source(config.cookie_source.clone()).await;
            Ok::<RuntimeBundle, String>(RuntimeBundle {
                handle: runtime.handle().clone(),
                queue,
                resolver,
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

        let (staging_sender, staging_receiver) = std_mpsc::channel();

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
            resolver: bundle.resolver,
            sidecar: bundle.sidecar,
            staging: staging::StagingState::Idle,
            staging_sender,
            staging_receiver,
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

    fn start_staging(&mut self, url: String) {
        let url = url.trim().to_owned();
        if url.is_empty() {
            return;
        }
        self.staging = staging::StagingState::Probing {
            url: url.clone(),
            started_at: std::time::Instant::now(),
        };
        let resolver = self.resolver.clone();
        let sender = self.staging_sender.clone();
        let cookie_source = self.config.cookie_source.clone();
        self.handle.spawn(async move {
            let cookie_ref = match &cookie_source {
                CookieSource::None => None,
                source => Some(source),
            };
            match resolver.resolve_with_cookies(&url, cookie_ref).await {
                Ok(manifest) => {
                    let thumbnail = if let Some(ref thumb_url) = manifest.thumbnail_url {
                        let client = match reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(5))
                            .build()
                        {
                            Ok(c) => c,
                            Err(_) => reqwest::Client::new(),
                        };
                        match client.get(thumb_url).send().await {
                            Ok(resp) => match resp.bytes().await {
                                Ok(b) => Some(b.to_vec()),
                                Err(_) => None,
                            },
                            Err(_) => None,
                        }
                    } else {
                        None
                    };
                    let _ = sender.send(staging::StagingEvent::Success {
                        url,
                        manifest,
                        thumbnail,
                    });
                }
                Err(error) => {
                    let _ = sender.send(staging::StagingEvent::Error {
                        url,
                        error: error.to_string(),
                    });
                }
            }
        });
    }

    #[allow(dead_code)]
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
                    self.start_staging(self.input.clone());
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
        while let Ok(event) = self.staging_receiver.try_recv() {
            match event {
                staging::StagingEvent::Success {
                    url,
                    manifest,
                    thumbnail,
                } => {
                    if let staging::StagingState::Probing {
                        url: probing_url, ..
                    } = &self.staging
                    {
                        if probing_url == &url {
                            let default_format = match staging::select_default_format_id(&manifest) {
                                Some(f) => f,
                                None => "best".to_string(),
                            };
                            let stream_size = manifest
                                .streams
                                .iter()
                                .find(|s| s.format_id == default_format)
                                .and_then(|s| s.filesize_bytes);
                            let disk_check =
                                staging::check_disk_space(&self.config.download_dir, stream_size);
                            self.staging = staging::StagingState::Staged {
                                url,
                                manifest,
                                thumbnail,
                                selected_format_id: default_format,
                                disk_check,
                            };
                        }
                    }
                }
                staging::StagingEvent::Error { url, error } => {
                    if let staging::StagingState::Probing {
                        url: probing_url, ..
                    } = &self.staging
                    {
                        if probing_url == &url {
                            self.staging = staging::StagingState::Error { url, error };
                        }
                    }
                }
            }
        }
        while let Ok(url) = self.clipboard_receiver.try_recv() {
            if self.auto_capture {
                self.start_staging(url);
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
                self.start_staging(file.name);
            } else if let Some(path) = file.path {
                if path.to_string_lossy().starts_with("http://")
                    || path.to_string_lossy().starts_with("https://")
                {
                    self.start_staging(path.to_string_lossy().into_owned());
                    continue;
                }
                match std::fs::read_to_string(&path) {
                    Ok(contents) => self.start_staging(contents),
                    Err(error) => set_status(
                        &self.status,
                        &format!("Could not read dropped file: {error}"),
                    ),
                }
            } else if let Some(bytes) = file.bytes {
                match String::from_utf8(bytes.to_vec()) {
                    Ok(contents) => self.start_staging(contents),
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
                self.start_staging(text);
            }
        }
    }
}

impl eframe::App for RustDlApp {
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        if visuals.dark_mode {
            [0.118, 0.118, 0.118, 1.0]
        } else {
            [0.965, 0.965, 0.965, 1.0]
        }
    }

    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
        self.handle_dropped_input(context);
        let mut input_focused = false;
        egui::TopBottomPanel::top("top_header")
            .exact_height(theme::HEADER_HEIGHT)
            .frame(egui::Frame::none().fill(egui::Color32::TRANSPARENT))
            .show(context, |ui| {
                theme::render_macos_top_header(ui, "rust-dl");
            });
        egui::TopBottomPanel::top("top_bar").show(context, |ui| {
            ui.horizontal(|ui| {
                ui.label(status_text(&self.status));
            });
            ui.horizontal(|ui| {
                let input = ui.add(
                    egui::TextEdit::singleline(&mut self.input)
                        .hint_text("Paste a media URL")
                        .desired_width(500.0),
                );
                input_focused = input.has_focus();
                if (input.lost_focus() || input_focused) && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    let text = self.input.trim().to_owned();
                    if !text.is_empty() {
                        self.start_staging(text);
                    }
                }
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
            } else {
                if !self.staging.is_idle() {
                    let queue = self.queue.clone();
                    let handle = self.handle.clone();
                    let allow_playlists = self.allow_playlists;
                    staging::render_staging_card(ui, &mut self.staging, |url, format_id| {
                        handle.spawn(async move {
                            match queue.enqueue_batch_for_selection(url.clone(), allow_playlists).await {
                                Ok(job_ids) => {
                                    for id in job_ids {
                                        let _ = queue.start_job(id, Some(format_id.clone()));
                                    }
                                }
                                Err(_) => {
                                    let _ = queue.enqueue_batch_auto(url, None, false, allow_playlists).await;
                                }
                            }
                        });
                    });
                    ui.add_space(12.0);
                }

                if self.jobs.is_empty() && self.staging.is_idle() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(60.0);
                        ui.heading(RichText::new("rust-dl").size(24.0).strong());
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new("Ultra-lightweight macOS Media Downloader")
                                .size(14.0)
                                .color(theme::TEXT_SECONDARY_DARK),
                        );
                        ui.add_space(20.0);
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("📋 Paste & Download (⌘V)").size(14.0),
                                )
                                .min_size(egui::vec2(220.0, 38.0)),
                            )
                            .clicked()
                        {
                            self.paste_and_download();
                        }
                        ui.add_space(16.0);
                        ui.label(
                            RichText::new(
                                "Supports YouTube (4K/1080p DASH with audio), Instagram, and direct media URLs",
                            )
                            .size(12.0)
                            .color(theme::TEXT_SECONDARY_DARK),
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("🎬 Auto DASH Video + Audio Muxing")
                                    .size(11.0)
                                    .color(theme::COLOR_ACCENT_BLUE),
                            );
                            ui.label(
                                RichText::new("•")
                                    .size(11.0)
                                    .color(theme::TEXT_SECONDARY_DARK),
                            );
                            ui.label(
                                RichText::new("⚡ Multi-chunk Parallel Range Workers")
                                    .size(11.0)
                                    .color(theme::COLOR_ACCENT_GREEN),
                            );
                        });
                    });
                } else if !self.jobs.is_empty() {
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
                        let queue = self.queue.clone();
                        let handle = self.handle.clone();
                        let on_retry = {
                            let queue = queue.clone();
                            let handle = handle.clone();
                            move |id| {
                                handle.spawn(async move {
                                    let _ = queue.retry_job(id);
                                });
                            }
                        };
                        let on_cancel = {
                            let queue = queue.clone();
                            let handle = handle.clone();
                            move |id| {
                                handle.spawn(async move {
                                    let _ = queue.cancel_job(id);
                                });
                            }
                        };
                        let on_remove = {
                            let queue = queue.clone();
                            let handle = handle.clone();
                            move |id| {
                                handle.spawn(async move {
                                    let _ = queue.remove_job(id);
                                });
                            }
                        };
                        let on_open_cookies = {
                            let status = Arc::clone(&self.status);
                            move || {
                                set_status(&status, "Select a Browser Cookie source in Settings above");
                            }
                        };

                        queue_view::render_queue_card_full(
                            ui,
                            &job,
                            on_retry,
                            on_cancel,
                            on_remove,
                            on_open_cookies,
                        );
                    }
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
                        self.start_staging(entry.url.clone());
                    }
                });
            });
        }
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
