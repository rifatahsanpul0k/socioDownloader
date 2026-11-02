#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rust_dl_core::downloader::DownloadEngine;
use rust_dl_core::queue::{JobStatus, QueueManager};
use rust_dl_core::{Resolver, SidecarManager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[tokio::test]
async fn queue_limits_active_downloads_and_reaches_completion(
) -> Result<(), Box<dyn std::error::Error>> {
    let state = ServerState::default();
    let (address, server) = start_server(state.clone(), false).await?;
    let directory = temporary_directory("queue-completion").await?;
    let manager = queue_manager(&directory).await?;
    let first = manager
        .enqueue_auto(format!("http://{address}/one.mp4"), false)
        .await?;
    let second = manager
        .enqueue_auto(format!("http://{address}/two.mp4"), false)
        .await?;
    let third = manager
        .enqueue_auto(format!("http://{address}/three.mp4"), false)
        .await?;

    wait_for_terminal(&manager, &[first, second, third]).await?;
    assert_eq!(state.max_active.load(Ordering::SeqCst), 1);
    for id in [first, second, third] {
        let job = manager
            .snapshot()
            .await
            .into_iter()
            .find(|job| job.id == id);
        assert!(matches!(
            job.map(|job| job.status),
            Some(JobStatus::Completed { .. })
        ));
    }

    server.abort();
    remove_directory(&directory).await;
    Ok(())
}

#[tokio::test]
async fn queue_cancellation_transitions_job_and_cleans_output_parts(
) -> Result<(), Box<dyn std::error::Error>> {
    let state = ServerState::default();
    let (address, server) = start_server(state, true).await?;
    let directory = temporary_directory("queue-cancel").await?;
    let manager = queue_manager(&directory).await?;
    let id = manager
        .enqueue_auto(format!("http://{address}/cancel.mp4"), false)
        .await?;

    wait_for_downloading(&manager, id).await?;
    manager.cancel_job(id)?;
    wait_for_status(&manager, id, |status| {
        matches!(status, JobStatus::Cancelled)
    })
    .await?;
    let mut entries = tokio::fs::read_dir(&directory).await?;
    assert!(entries.next_entry().await?.is_none());

    server.abort();
    remove_directory(&directory).await;
    Ok(())
}

async fn queue_manager(directory: &Path) -> Result<QueueManager, Box<dyn std::error::Error>> {
    let sidecar = SidecarManager::new()?;
    let resolver = Resolver::new(sidecar.clone())?;
    let engine = DownloadEngine::new(sidecar)?;
    Ok(QueueManager::new(
        resolver,
        engine,
        directory.to_path_buf(),
        1,
    ))
}

async fn wait_for_terminal(
    manager: &QueueManager,
    ids: &[u64],
) -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..100 {
        let jobs = manager.snapshot().await;
        if ids.iter().all(|id| {
            jobs.iter()
                .find(|job| job.id == *id)
                .map(|job| {
                    matches!(
                        job.status,
                        JobStatus::Completed { .. }
                            | JobStatus::Failed { .. }
                            | JobStatus::Cancelled
                    )
                })
                .unwrap_or(false)
        }) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Err("queue jobs did not reach terminal states".into())
}

async fn wait_for_downloading(
    manager: &QueueManager,
    id: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    wait_for_status(manager, id, |status| {
        matches!(status, JobStatus::Downloading { .. })
    })
    .await
}

async fn wait_for_status<F>(
    manager: &QueueManager,
    id: u64,
    predicate: F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: Fn(&JobStatus) -> bool,
{
    for _ in 0..120 {
        let job = manager
            .snapshot()
            .await
            .into_iter()
            .find(|job| job.id == id);
        if let Some(job) = job {
            if predicate(&job.status) {
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Err("job did not reach the expected state".into())
}

#[derive(Clone, Default)]
struct ServerState {
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
}

async fn start_server(
    state: ServerState,
    slow: bool,
) -> Result<(std::net::SocketAddr, tokio::task::JoinHandle<()>), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move {
        loop {
            let accepted = listener.accept().await;
            let Ok((stream, _)) = accepted else { break };
            let state = state.clone();
            tokio::spawn(async move {
                let _ = respond(stream, state, slow).await;
            });
        }
    });
    Ok((address, server))
}

async fn respond(
    mut stream: TcpStream,
    state: ServerState,
    slow: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut request = [0_u8; 4096];
    let size = stream.read(&mut request).await?;
    let request = String::from_utf8_lossy(&request[..size]);
    let body = vec![b'x'; 1024];
    if request.starts_with("HEAD ") {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: video/mp4\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(response.as_bytes()).await?;
        return Ok(());
    }
    let active = state.active.fetch_add(1, Ordering::SeqCst) + 1;
    state.max_active.fetch_max(active, Ordering::SeqCst);
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: video/mp4\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    if slow {
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let _ = stream.write_all(&body).await;
    state.active.fetch_sub(1, Ordering::SeqCst);
    Ok(())
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
