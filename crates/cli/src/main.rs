use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Instant;

use rust_dl_core::downloader::types::{DownloadPhase, DownloadProgress};
use rust_dl_core::downloader::{DownloadEngine, DownloadError};
use rust_dl_core::{
    BrowserKind, CookieSource, DownloadProgress as SidecarProgress, MediaManifest, MediaStream,
    Resolver, SidecarManager, StreamDelivery,
};
use tokio::sync::{mpsc, watch};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args().skip(1);
    match arguments.next().as_deref() {
        None | Some("bootstrap") => bootstrap().await?,
        Some("resolve") => {
            let url = arguments.next().ok_or("usage: rust-dl-cli resolve <URL>")?;
            let (cookie_source, allow_playlist) = parse_resolve_flags(arguments)?;
            resolve(&url, cookie_source, allow_playlist).await?;
        }
        Some("download") => download_command(arguments).await?,
        Some("update") => update_command().await?,
        Some("bundle-app") => bundle_app_command(arguments)?,
        Some(command) => {
            return Err(
                format!("unknown command '{command}'; use 'bootstrap', 'resolve', 'download', 'update', or 'bundle-app'").into(),
            );
        }
    }
    Ok(())
}

async fn bootstrap() -> Result<(), Box<dyn std::error::Error>> {
    let manager = SidecarManager::new()?;
    let (progress_tx, mut progress_rx) = mpsc::channel(32);
    let mut bootstrap = Box::pin(manager.ensure_binaries(progress_tx));

    println!("Preparing rust-dl media tools...");
    loop {
        tokio::select! {
            result = &mut bootstrap => {
                result?;
                break;
            }
            event = progress_rx.recv() => {
                if let Some(event) = event {
                    display_progress(event)?;
                }
            }
        }
    }
    while let Ok(event) = progress_rx.try_recv() {
        display_progress(event)?;
    }

    let versions = manager.verify_binaries().await?;
    println!("yt-dlp: {}", versions.yt_dlp);
    println!("ffmpeg: {}", versions.ffmpeg);
    Ok(())
}

async fn resolve(
    url: &str,
    cookie_source: CookieSource,
    allow_playlist: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let manager = SidecarManager::new()?;
    let (progress_tx, progress_rx) = mpsc::channel(1);
    drop(progress_rx);
    manager.ensure_binaries(progress_tx).await?;

    let resolver = Resolver::new(manager)?;
    let started = Instant::now();
    let manifests = resolver
        .resolve_all(url, &cookie_source, allow_playlist)
        .await?;
    let elapsed = started.elapsed().as_millis();
    for (index, manifest) in manifests.iter().enumerate() {
        if manifests.len() > 1 {
            println!("Item {} of {}", index + 1, manifests.len());
        }
        print_manifest(manifest, elapsed);
    }
    Ok(())
}

async fn download_command(
    mut arguments: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let url = arguments
        .next()
        .ok_or("usage: rust-dl-cli download <URL> [--format <FORMAT_ID>] [--output <DIR>]")?;
    let mut requested_format = None;
    let mut output_directory = None;
    let mut cookie_source = CookieSource::None;
    let mut allow_playlist = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--format" => {
                requested_format = Some(arguments.next().ok_or("--format requires a format ID")?);
            }
            "--output" => {
                output_directory = Some(
                    arguments
                        .next()
                        .map(PathBuf::from)
                        .ok_or("--output requires a directory")?,
                );
            }
            "--cookies-from-browser" => {
                let browser = arguments
                    .next()
                    .ok_or("--cookies-from-browser requires a browser")?;
                cookie_source = CookieSource::Browser(BrowserKind::from_str(&browser)?);
            }
            "--cookies" => {
                let path = arguments.next().ok_or("--cookies requires a file")?;
                cookie_source = CookieSource::CookieFile(PathBuf::from(path));
            }
            "--playlist" => allow_playlist = true,
            unknown => return Err(format!("unknown download option '{unknown}'").into()),
        }
    }

    let manager = SidecarManager::new()?;
    let (bootstrap_sender, bootstrap_receiver) = mpsc::channel(1);
    drop(bootstrap_receiver);
    manager.ensure_binaries(bootstrap_sender).await?;
    let resolver = Resolver::new(manager.clone())?;
    let manifests = resolver
        .resolve_all(&url, &cookie_source, allow_playlist)
        .await?;
    let engine = DownloadEngine::new(manager)?;
    for manifest in manifests {
        let stream = choose_stream(&manifest, requested_format.as_deref())?;
        download_manifest(&engine, &manifest, stream, output_directory.as_deref()).await?;
    }
    Ok(())
}

async fn download_manifest(
    engine: &DownloadEngine,
    manifest: &MediaManifest,
    stream: &MediaStream,
    output_directory: Option<&std::path::Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (progress_sender, mut progress_receiver) = mpsc::channel(128);
    let (cancel_sender, cancel_receiver) = watch::channel(false);
    let started = Instant::now();
    let mut download = Box::pin(engine.download(
        manifest,
        stream,
        output_directory,
        cancel_receiver,
        progress_sender,
    ));
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    let mut cancellation_requested = false;
    let result = loop {
        tokio::select! {
            result = &mut download => break result,
            event = progress_receiver.recv() => {
                if let Some(event) = event {
                    render_download_progress(&event)?;
                }
            }
            _ = &mut ctrl_c, if !cancellation_requested => {
                cancellation_requested = true;
                let _ = cancel_sender.send(true);
                println!("\nCancellation requested; cleaning up temporary files...");
            }
        }
    };
    while let Ok(event) = progress_receiver.try_recv() {
        render_download_progress(&event)?;
    }

    match result {
        Ok(path) => println!(
            "Saved: {} ({:.1}s)",
            path.display(),
            started.elapsed().as_secs_f64()
        ),
        Err(DownloadError::Cancelled) => println!("Download cancelled."),
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn parse_resolve_flags(
    mut arguments: impl Iterator<Item = String>,
) -> Result<(CookieSource, bool), Box<dyn std::error::Error>> {
    let mut source = CookieSource::None;
    let mut allow_playlist = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--cookies-from-browser" => {
                let browser = arguments
                    .next()
                    .ok_or("--cookies-from-browser requires a browser")?;
                source = CookieSource::Browser(BrowserKind::from_str(&browser)?);
            }
            "--cookies" => {
                let path = arguments.next().ok_or("--cookies requires a file")?;
                source = CookieSource::CookieFile(PathBuf::from(path));
            }
            "--playlist" => allow_playlist = true,
            unknown => return Err(format!("unknown option '{unknown}'").into()),
        }
    }
    Ok((source, allow_playlist))
}

async fn update_command() -> Result<(), Box<dyn std::error::Error>> {
    let manager = SidecarManager::new()?;
    let report = manager.update_ytdlp().await?;
    let state = if report.updated {
        "updated"
    } else {
        "up to date"
    };
    println!(
        "yt-dlp {} -> {} ({state})",
        report.previous_version, report.current_version
    );
    if !report.output_summary.is_empty() {
        println!("{}", report.output_summary);
    }
    Ok(())
}

fn bundle_app_command(
    mut arguments: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut output_dir = PathBuf::from(".");
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--output-dir" => {
                output_dir = PathBuf::from(
                    arguments
                        .next()
                        .ok_or("--output-dir requires a directory")?,
                );
            }
            unknown => return Err(format!("unknown bundle-app option '{unknown}'").into()),
        }
    }
    let gui_binary = std::env::current_dir()?.join("target/release/rust-dl-gui");
    let app =
        rust_dl_core::bundler::bundle_app(&gui_binary, &output_dir, env!("CARGO_PKG_VERSION"))?;
    println!("Created {}", app.display());
    Ok(())
}

fn choose_stream<'a>(
    manifest: &'a MediaManifest,
    requested_format: Option<&str>,
) -> Result<&'a MediaStream, Box<dyn std::error::Error>> {
    match requested_format {
        Some(format_id) => manifest
            .streams
            .iter()
            .find(|stream| stream.format_id == format_id)
            .ok_or_else(|| format!("format '{format_id}' was not found").into()),
        None => manifest
            .streams
            .first()
            .ok_or_else(|| "resolver returned no streams".into()),
    }
}

fn render_download_progress(progress: &DownloadProgress) -> io::Result<()> {
    let phase = match &progress.phase {
        DownloadPhase::Probing => "Probing",
        DownloadPhase::Downloading => "Downloading",
        DownloadPhase::Muxing => "Muxing",
        DownloadPhase::Completed { .. } => "Completed",
        DownloadPhase::Failed { .. } => "Failed",
        DownloadPhase::Cancelled => "Cancelled",
    };
    let percentage = progress
        .total_bytes
        .filter(|total| *total > 0)
        .map(|total| {
            format!(
                "{:>3.0}%",
                progress.downloaded_bytes as f64 * 100.0 / total as f64
            )
        })
        .unwrap_or_else(|| " --%".to_owned());
    let total = progress
        .total_bytes
        .map(|bytes| format!("{:.1} MB", bytes as f64 / 1_048_576.0))
        .unwrap_or_else(|| "? MB".to_owned());
    let eta = progress
        .eta_secs
        .map(|seconds| format!("{seconds}s"))
        .unwrap_or_else(|| "--".to_owned());
    print!(
        "\r{phase:<11} {percentage} {:>8.1}/{:<8} MB {:>7.2} MB/s ETA {eta:<5}",
        progress.downloaded_bytes as f64 / 1_048_576.0,
        total,
        progress.speed_bytes_per_sec / 1_048_576.0,
    );
    io::stdout().flush()
}

fn print_manifest(manifest: &MediaManifest, elapsed_ms: u128) {
    println!("Title:    {}", manifest.title);
    println!("Platform: {}", manifest.platform);
    println!("Uploader: {}", manifest.uploader);
    println!("Resolved: {elapsed_ms} ms");
    println!();
    println!(
        "{:<18} {:<12} {:<12} {:<12}",
        "FORMAT", "RESOLUTION", "SIZE", "DELIVERY"
    );
    println!("{}", "-".repeat(58));
    for stream in &manifest.streams {
        let resolution = stream.resolution.as_deref().unwrap_or("-");
        let size = stream
            .filesize_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "-".to_owned());
        let delivery = match stream.delivery {
            StreamDelivery::Direct { .. } => "direct",
            StreamDelivery::SplitDash { .. } => "split-dash",
            StreamDelivery::Hls { .. } => "HLS",
        };
        println!(
            "{:<18} {:<12} {:<12} {:<12}",
            stream.format_id, resolution, size, delivery
        );
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn display_progress(event: SidecarProgress) -> io::Result<()> {
    match event {
        SidecarProgress::Started {
            binary,
            total_bytes,
        } => {
            match total_bytes {
                Some(total) => print!("Downloading {binary} ({total} bytes)..."),
                None => print!("Downloading {binary}..."),
            }
            io::stdout().flush()
        }
        SidecarProgress::Bytes {
            downloaded_bytes,
            total_bytes,
            ..
        } => {
            match total_bytes {
                Some(total) if total > 0 => {
                    let percent = downloaded_bytes.saturating_mul(100) / total;
                    print!("\r  {percent:>3}% ({downloaded_bytes}/{total} bytes)");
                }
                _ => print!("\r  {downloaded_bytes} bytes"),
            }
            io::stdout().flush()
        }
        SidecarProgress::Completed { binary, .. } => {
            println!("\r  {binary} ready");
            Ok(())
        }
    }
}
