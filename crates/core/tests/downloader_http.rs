use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rust_dl_core::downloader::fs::part_path;
use rust_dl_core::downloader::{DownloadEngine, DownloadError};
use rust_dl_core::{MediaManifest, MediaStream, Platform, SidecarManager, StreamDelivery};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};

#[tokio::test]
async fn range_engine_assembles_parallel_segments_with_positioned_writes(
) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = test_bytes(5 * 1024 * 1024 + 137);
    let (address, server) = start_server(bytes.clone(), false).await?;
    let directory = temporary_directory("range").await?;
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let (progress_tx, mut progress_rx) = mpsc::channel(64);
    let manager = SidecarManager::new()?;
    let engine = DownloadEngine::new(manager)?;
    let manifest = direct_manifest(format!("http://{address}/media.mp4"), bytes.len() as u64);

    let output = engine
        .download(
            &manifest,
            &manifest.streams[0],
            Some(&directory),
            cancel_rx,
            progress_tx,
        )
        .await?;
    assert_eq!(tokio::fs::read(&output).await?, bytes);
    assert!(!tokio::fs::metadata(part_path(&output)).await.is_ok());
    assert!(progress_rx.try_recv().is_ok());

    let _ = cancel_tx;
    server.abort();
    remove_directory(&directory).await;
    Ok(())
}

#[tokio::test]
async fn cancellation_removes_inflight_part_files() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = test_bytes(5 * 1024 * 1024 + 137);
    let (address, server) = start_server(bytes, true).await?;
    let directory = temporary_directory("cancel").await?;
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let (progress_tx, _progress_rx) = mpsc::channel(64);
    let manager = SidecarManager::new()?;
    let engine = DownloadEngine::new(manager)?;
    let manifest = direct_manifest(format!("http://{address}/media.mp4"), 5 * 1024 * 1024 + 137);
    let output_directory = directory.clone();
    let task = tokio::spawn(async move {
        engine
            .download(
                &manifest,
                &manifest.streams[0],
                Some(&output_directory),
                cancel_rx,
                progress_tx,
            )
            .await
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    cancel_tx.send(true)?;
    let result = tokio::time::timeout(Duration::from_secs(5), task).await??;
    assert!(matches!(result, Err(DownloadError::Cancelled)));
    let mut entries = tokio::fs::read_dir(&directory).await?;
    assert!(entries.next_entry().await?.is_none());

    server.abort();
    remove_directory(&directory).await;
    Ok(())
}

fn direct_manifest(url: String, size: u64) -> MediaManifest {
    MediaManifest {
        id: "loopback".to_owned(),
        title: "Loopback media".to_owned(),
        uploader: "test".to_owned(),
        duration_secs: None,
        thumbnail_url: None,
        platform: Platform::Generic,
        streams: vec![MediaStream {
            format_id: "direct".to_owned(),
            delivery: StreamDelivery::Direct { url },
            container: "mp4".to_owned(),
            resolution: Some("1920x1080".to_owned()),
            fps: None,
            filesize_bytes: Some(size),
            vcodec: Some("h264".to_owned()),
            acodec: Some("aac".to_owned()),
            headers: HashMap::new(),
        }],
    }
}

async fn start_server(
    bytes: Vec<u8>,
    delay_body: bool,
) -> Result<(std::net::SocketAddr, tokio::task::JoinHandle<()>), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move {
        loop {
            let accepted = listener.accept().await;
            let Ok((stream, _)) = accepted else { break };
            let body = bytes.clone();
            tokio::spawn(async move {
                let _ = respond(stream, body, delay_body).await;
            });
        }
    });
    Ok((address, server))
}

async fn respond(
    mut stream: TcpStream,
    body: Vec<u8>,
    delay_body: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut request = vec![0_u8; 8192];
    let size = stream.read(&mut request).await?;
    let request = String::from_utf8_lossy(&request[..size]);
    if request.starts_with("HEAD ") {
        let response = format!(
            "HTTP/1.1 200 OK\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nContent-Type: video/mp4\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(response.as_bytes()).await?;
        return Ok(());
    }

    if delay_body {
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let last_byte = body.len().saturating_sub(1) as u64;
    let (start, end) = parse_range(&request).unwrap_or((0, last_byte));
    let end = end.min(last_byte);
    let start = start.min(end);
    let payload = &body[start as usize..=end as usize];
    let header = format!(
        "HTTP/1.1 206 Partial Content\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{}\r\nContent-Length: {}\r\nContent-Type: video/mp4\r\nConnection: close\r\n\r\n",
        body.len(),
        payload.len()
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(payload).await?;
    Ok(())
}

fn parse_range(request: &str) -> Option<(u64, u64)> {
    let value = request
        .lines()
        .find(|line| line.to_ascii_lowercase().starts_with("range:"))?
        .split_once('=')?
        .1
        .trim();
    let (start, end) = value.split_once('-')?;
    Some((start.parse().ok()?, end.parse().ok()?))
}

fn test_bytes(length: usize) -> Vec<u8> {
    (0..length).map(|index| (index % 251) as u8).collect()
}

async fn temporary_directory(name: &str) -> Result<PathBuf, std::io::Error> {
    let directory = std::env::temp_dir().join(format!("rust-dl-{name}-{}", std::process::id()));
    match tokio::fs::remove_dir_all(&directory).await {
        Ok(()) | Err(_) => {}
    }
    tokio::fs::create_dir_all(&directory).await?;
    Ok(directory)
}

async fn remove_directory(directory: &Path) {
    match tokio::fs::remove_dir_all(directory).await {
        Ok(()) | Err(_) => {}
    }
}
