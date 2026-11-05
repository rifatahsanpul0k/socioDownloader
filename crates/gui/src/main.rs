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

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SimpleFormatMode {
    #[default]
    HdVideo,
    Max4k,
    AudioOnly,
}

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum QualityMode {
    Best,
    Max1080,
    Max720,
    AudioOnly,
    Ask,
}

#[allow(dead_code)]
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
    #[allow(dead_code)]
    sidecar: SidecarManager,
    staging: staging::StagingState,
    staging_sender: std_mpsc::Sender<staging::StagingEvent>,
    staging_receiver: std_mpsc::Receiver<staging::StagingEvent>,
    input: String,
    quality: QualityMode,
    format_mode: SimpleFormatMode,
    show_help: bool,
    show_settings: bool,
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
        "Media Saver",
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

        let status = Arc::new(Mutex::new("Ready".to_owned()));
        let bootstrap_status = Arc::clone(&status);
        let sidecar = bundle.sidecar.clone();
        bundle.handle.spawn(async move {
            set_status(&bootstrap_status, "First-time setup...");
            let (progress_sender, progress_receiver) = mpsc::channel(1);
            drop(progress_receiver);
            let result = sidecar.ensure_binaries(progress_sender).await;
            match result {
                Ok(()) => set_status(&bootstrap_status, "Ready"),
                Err(_) => set_status(&bootstrap_status, "Setup error. Check internet connection."),
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
            format_mode: SimpleFormatMode::HdVideo,
            show_help: false,
            show_settings: false,
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

    fn start_download(&mut self, text: String) {
        let clean = match rust_dl_core::find_url_in_text(&text) {
            Some(url) => url.to_owned(),
            None => text.trim().to_owned(),
        };
        if clean.is_empty() {
            return;
        }
        let queue = self.queue.clone();
        let format_mode = self.format_mode;
        let allow_playlists = self.allow_playlists;
        let status = Arc::clone(&self.status);
        self.handle.spawn(async move {
            let (max_height, audio_only) = match format_mode {
                SimpleFormatMode::HdVideo => (Some(1080), false),
                SimpleFormatMode::Max4k => (None, false),
                SimpleFormatMode::AudioOnly => (None, true),
            };
            if let Err(error) = queue
                .enqueue_batch_auto(clean, max_height, audio_only, allow_playlists)
                .await
            {
                set_status(&status, &error.to_string());
            }
        });
    }

    fn paste_and_download(&mut self) {
        match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text()) {
            Ok(text) => {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    self.input.clear();
                    self.start_download(trimmed.to_owned());
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
                self.start_download(url);
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

    #[allow(dead_code)]
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

    #[allow(dead_code)]
    fn apply_allow_playlists(&mut self, enabled: bool) {
        if self.allow_playlists == enabled {
            return;
        }
        self.allow_playlists = enabled;
        self.config.allow_playlists = enabled;
        self.save_config();
    }

    #[allow(dead_code)]
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
                self.start_download(file.name);
            } else if let Some(path) = file.path {
                if path.to_string_lossy().starts_with("http://")
                    || path.to_string_lossy().starts_with("https://")
                {
                    self.start_download(path.to_string_lossy().into_owned());
                    continue;
                }
                match std::fs::read_to_string(&path) {
                    Ok(contents) => self.start_download(contents),
                    Err(error) => set_status(
                        &self.status,
                        &format!("Could not read dropped file: {error}"),
                    ),
                }
            } else if let Some(bytes) = file.bytes {
                match String::from_utf8(bytes.to_vec()) {
                    Ok(contents) => self.start_download(contents),
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
                self.start_download(text);
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
                theme::render_macos_top_header(ui, "Media Saver");
            });

        egui::TopBottomPanel::top("top_bar").show(context, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Media Saver").strong().size(15.0));
                ui.add_space(8.0);

                let status = status_text(&self.status);
                let (dot_color, dot_label) = if status == "Ready" {
                    (theme::COLOR_ACCENT_GREEN, "● Ready")
                } else if status.contains("setup")
                    || status.contains("Preparing")
                    || status.contains("Checking")
                {
                    (theme::COLOR_ACCENT_INDIGO, "● First-time setup...")
                } else {
                    (theme::COLOR_ACCENT_AMBER, status.as_str())
                };
                ui.label(RichText::new(dot_label).color(dot_color).size(12.0));

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("⚙ Settings").clicked() {
                        self.show_settings = !self.show_settings;
                    }
                    if ui.button("❓ Help").clicked() {
                        self.show_help = !self.show_help;
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
            });
            ui.add_space(2.0);
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
                // Hero Action Card (Heuristics #1, #2, #5, #7, #8)
                egui::Frame::none()
                    .fill(if ui.visuals().dark_mode {
                        egui::Color32::from_rgb(34, 34, 38)
                    } else {
                        egui::Color32::from_rgb(255, 255, 255)
                    })
                    .rounding(egui::Rounding::same(12.0))
                    .inner_margin(egui::Margin::same(16.0))
                    .stroke(egui::Stroke::new(
                        1.0_f32,
                        if ui.visuals().dark_mode {
                            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 25)
                        } else {
                            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 20)
                        },
                    ))
                    .shadow(egui::epaint::Shadow {
                        offset: egui::vec2(0.0, 2.0),
                        blur: 8.0,
                        spread: 0.0,
                        color: egui::Color32::from_black_alpha(25),
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let available_w = ui.available_width() - 175.0;
                            let input = ui.add(
                                egui::TextEdit::singleline(&mut self.input)
                                    .hint_text("Paste any YouTube, Instagram, Pinterest, Facebook, or GitHub link...")
                                    .desired_width(available_w.max(260.0)),
                            );
                            input_focused = input.has_focus();
                            if (input.lost_focus() || input_focused)
                                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                            {
                                let text = self.input.trim().to_owned();
                                if !text.is_empty() {
                                    self.input.clear();
                                    self.start_download(text);
                                } else {
                                    self.paste_and_download();
                                }
                            }

                            let btn = egui::Button::new(
                                RichText::new("📋 Paste & Download").strong().color(egui::Color32::WHITE),
                            )
                            .fill(theme::COLOR_ACCENT_BLUE)
                            .rounding(egui::Rounding::same(8.0))
                            .min_size(egui::vec2(165.0, 32.0));
                            if ui.add(btn).clicked() {
                                if self.input.trim().is_empty() {
                                    self.paste_and_download();
                                } else {
                                    let text = std::mem::take(&mut self.input);
                                    self.start_download(text);
                                }
                            }
                        });

                        ui.add_space(10.0);

                        // 3 Simple Format Pills
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Format:")
                                    .size(12.0)
                                    .color(theme::TEXT_SECONDARY_DARK),
                            );
                            ui.add_space(4.0);

                            for (mode, label) in [
                                (SimpleFormatMode::HdVideo, "🎬 Video (HD)"),
                                (SimpleFormatMode::Max4k, "✨ Video (4K Max)"),
                                (SimpleFormatMode::AudioOnly, "🎵 Audio Only"),
                            ] {
                                let is_selected = self.format_mode == mode;
                                let bg_color = if is_selected {
                                    theme::COLOR_ACCENT_BLUE
                                } else if ui.visuals().dark_mode {
                                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 18)
                                } else {
                                    egui::Color32::from_rgba_unmultiplied(0, 0, 0, 15)
                                };
                                let text_color = if is_selected {
                                    egui::Color32::WHITE
                                } else {
                                    ui.visuals().text_color()
                                };

                                let pill = egui::Button::new(
                                    RichText::new(label).size(12.0).color(text_color),
                                )
                                .fill(bg_color)
                                .rounding(egui::Rounding::same(14.0));
                                if ui.add(pill).clicked() {
                                    self.format_mode = mode;
                                }
                            }
                        });
                    });

                ui.add_space(16.0);

                if self.jobs.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(40.0);
                        ui.label(
                            RichText::new("⬇")
                                .size(48.0)
                                .color(theme::COLOR_ACCENT_BLUE),
                        );
                        ui.add_space(10.0);
                        ui.heading(RichText::new("Ready to Download").size(22.0).strong());
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(
                                "Copy any video or file link and press ⌘V, or paste it above.",
                            )
                            .size(13.5)
                            .color(theme::TEXT_SECONDARY_DARK),
                        );
                        ui.add_space(24.0);

                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                            for (icon, name, color) in [
                                ("▶", "YouTube (HD/4K with Sound)", egui::Color32::from_rgb(255, 59, 48)),
                                ("📷", "Instagram Reels & Posts", egui::Color32::from_rgb(175, 82, 222)),
                                ("📌", "Pinterest Pins & Media", egui::Color32::from_rgb(230, 0, 35)),
                                ("📘", "Facebook Videos", egui::Color32::from_rgb(0, 122, 255)),
                                ("🐙", "GitHub Releases, Files & Zips", egui::Color32::from_rgb(140, 95, 230)),
                                ("📦", "Direct Media & Files", egui::Color32::from_rgb(142, 142, 147)),
                            ] {
                                egui::Frame::none()
                                    .fill(if ui.visuals().dark_mode {
                                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 12)
                                    } else {
                                        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 8)
                                    })
                                    .rounding(egui::Rounding::same(8.0))
                                    .inner_margin(egui::Margin::symmetric(10.0, 6.0))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(RichText::new(icon).color(color).size(13.0));
                                            ui.label(RichText::new(name).size(12.0));
                                        });
                                    });
                            }
                        });
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
                        ui.add_space(8.0);
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
                                set_status(&status, "Select your browser in Settings (⚙) to enable cookies");
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

        // Modals
        if self.show_help {
            egui::Window::new("Help & Tips")
                .collapsible(false)
                .resizable(false)
                .default_width(460.0)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(context, |ui| {
                    ui.spacing_mut().item_spacing.y = 10.0;
                    ui.label(RichText::new("How to Download").strong().size(14.0));
                    ui.label(
                        "Copy any video or file link from your web browser or another app. Then press ⌘V inside Media Saver or click 'Paste & Download'.",
                    );
                    ui.separator();
                    ui.label(RichText::new("Apple QuickTime Ready").strong().size(14.0));
                    ui.label(
                        "All downloaded videos are automatically packaged with crystal-clear AAC audio so they play immediately in macOS QuickTime Player and on your iPhone.",
                    );
                    ui.separator();
                    ui.label(RichText::new("Private & Age-Restricted Content").strong().size(14.0));
                    ui.label(
                        "If a video requires you to be logged in, click Settings (⚙) and select your web browser under 'Browser Cookies'. Media Saver will safely use your session to download it.",
                    );
                    ui.add_space(8.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Got It").clicked() {
                            self.show_help = false;
                        }
                    });
                });
        }

        if self.show_settings {
            egui::Window::new("Preferences")
                .collapsible(false)
                .resizable(false)
                .default_width(480.0)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(context, |ui| {
                    ui.spacing_mut().item_spacing.y = 12.0;

                    // Download folder
                    ui.label(RichText::new("Download Location").strong().size(13.5));
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(self.config.download_dir.display().to_string())
                                .size(12.0)
                                .color(theme::TEXT_SECONDARY_DARK),
                        );
                        if ui.button("Open in Finder").clicked() {
                            let _ = std::process::Command::new("open")
                                .arg(&self.config.download_dir)
                                .status();
                        }
                    });

                    ui.separator();

                    // Browser Cookies
                    ui.label(RichText::new("Browser Cookies (For Private Videos)").strong().size(13.5));
                    ui.label(
                        RichText::new("Select your web browser to download videos requiring sign-in.")
                            .size(11.5)
                            .color(theme::TEXT_SECONDARY_DARK),
                    );
                    let mut source = self.config.cookie_source.clone();
                    egui::ComboBox::from_id_salt("cookie_source")
                        .selected_text(cookie_source_label(&source))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut source, CookieSource::None, "Off (Default)");
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

                    ui.separator();

                    // Simultaneous Downloads
                    ui.label(RichText::new("Simultaneous Downloads").strong().size(13.5));
                    let mut concurrency = self.config.max_concurrent_downloads;
                    egui::ComboBox::from_id_salt("concurrency")
                        .selected_text(format!("{} at a time", concurrency))
                        .show_ui(ui, |ui| {
                            for value in 1..=8 {
                                ui.selectable_value(&mut concurrency, value, format!("{} at a time", value));
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

                    ui.separator();

                    // Auto-Capture Clipboard
                    let mut auto_capture = self.auto_capture;
                    if ui
                        .checkbox(
                            &mut auto_capture,
                            "Automatically detect copied media links in background",
                        )
                        .changed()
                    {
                        self.apply_auto_capture(auto_capture);
                    }

                    ui.add_space(8.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Done").clicked() {
                            self.show_settings = false;
                        }
                    });
                });
        }

        context.request_repaint_after(Duration::from_millis(100));
    }
}

impl RustDlApp {
    #[allow(dead_code)]
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

#[allow(dead_code)]
fn quality_from_preset(preset: &str) -> QualityMode {
    match preset {
        "1080p" => QualityMode::Max1080,
        "720p" => QualityMode::Max720,
        "audio" => QualityMode::AudioOnly,
        "ask" => QualityMode::Ask,
        _ => QualityMode::Best,
    }
}

#[allow(dead_code)]
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
