//! yt-dlp invocation and compact JSON-to-domain normalization.

use std::collections::{HashMap, HashSet};
use std::process::Stdio;

use serde::Deserialize;
use tokio::process::Command;

use super::sanitizer::{Platform, SanitizedUrl};
use super::types::{MediaManifest, MediaStream, StreamDelivery};
use super::ResolverError;
use crate::auth::CookieSource;
use crate::sidecar::{SidecarManager, YT_DLP_BINARY_NAME};

#[derive(Debug, Deserialize)]
struct RawManifest {
    #[serde(rename = "_type")]
    kind: Option<String>,
    id: Option<String>,
    title: Option<String>,
    uploader: Option<String>,
    duration: Option<f64>,
    thumbnail: Option<String>,
    #[serde(default)]
    http_headers: HashMap<String, String>,
    #[serde(default)]
    formats: Vec<RawFormat>,
    #[serde(default)]
    entries: Vec<Option<RawManifest>>,
}

#[derive(Debug, Deserialize)]
struct RawFormat {
    format_id: Option<String>,
    url: Option<String>,
    protocol: Option<String>,
    ext: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    resolution: Option<String>,
    fps: Option<f32>,
    filesize: Option<u64>,
    filesize_approx: Option<u64>,
    vcodec: Option<String>,
    acodec: Option<String>,
    abr: Option<f32>,
    #[serde(default)]
    http_headers: HashMap<String, String>,
}

#[derive(Debug, Clone)]
struct ParsedFormat {
    format_id: String,
    url: String,
    protocol: String,
    container: String,
    resolution: Option<String>,
    fps: Option<f32>,
    filesize_bytes: Option<u64>,
    vcodec: Option<String>,
    acodec: Option<String>,
    abr: Option<f32>,
    headers: HashMap<String, String>,
}

pub async fn resolve(
    sidecar: &SidecarManager,
    sanitized: &SanitizedUrl,
    cookie_source: Option<&CookieSource>,
) -> Result<MediaManifest, ResolverError> {
    resolve_with_options(sidecar, sanitized, cookie_source, false, 25)
        .await
        .and_then(|mut manifests| manifests.pop().ok_or(ResolverError::NoStreams))
}

pub async fn resolve_all(
    sidecar: &SidecarManager,
    sanitized: &SanitizedUrl,
    cookie_source: Option<&CookieSource>,
    max_playlist_items: usize,
) -> Result<Vec<MediaManifest>, ResolverError> {
    resolve_with_options(sidecar, sanitized, cookie_source, true, max_playlist_items).await
}

async fn resolve_with_options(
    sidecar: &SidecarManager,
    sanitized: &SanitizedUrl,
    cookie_source: Option<&CookieSource>,
    allow_playlist: bool,
    max_playlist_items: usize,
) -> Result<Vec<MediaManifest>, ResolverError> {
    let mut command = Command::new(sidecar.binary_path(YT_DLP_BINARY_NAME));
    command
        .args([
            "--dump-single-json",
            "--no-warnings",
            "--socket-timeout",
            "15",
            "--ffmpeg-location",
        ])
        .arg(sidecar.bin_dir())
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if allow_playlist {
        command.args(["--yes-playlist", "--playlist-end"]);
        command.arg(max_playlist_items.max(1).to_string());
    } else {
        command.arg("--no-playlist");
    }
    if let Some(cookie_source) = cookie_source {
        command.args(cookie_source.to_ytdlp_args());
    }
    command.arg(sanitized.url.as_str());
    let output = command.output().await.map_err(ResolverError::ProcessIo)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        if cookie_source.is_some() && is_cookie_extraction_error(&stderr) {
            let mut retry_cmd = Command::new(sidecar.binary_path(YT_DLP_BINARY_NAME));
            retry_cmd
                .args([
                    "--dump-single-json",
                    "--no-warnings",
                    "--socket-timeout",
                    "15",
                    "--ffmpeg-location",
                ])
                .arg(sidecar.bin_dir())
                .kill_on_drop(true)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if allow_playlist {
                retry_cmd.args(["--yes-playlist", "--playlist-end"]);
                retry_cmd.arg(max_playlist_items.max(1).to_string());
            } else {
                retry_cmd.arg("--no-playlist");
            }
            retry_cmd.arg(sanitized.url.as_str());
            if let Ok(retry_output) = retry_cmd.output().await {
                if retry_output.status.success() {
                    return parse_manifests(&retry_output.stdout, sanitized.platform);
                }
            }
        }
        if let Some(message) = authentication_message(&stderr) {
            return Err(ResolverError::AuthenticationRequired {
                platform: sanitized.platform,
                message,
            });
        }
        return Err(ResolverError::YtDlpFailed { stderr });
    }
    parse_manifests(&output.stdout, sanitized.platform)
}

fn is_cookie_extraction_error(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("could not extract cookies")
        || lower.contains("cookie database")
        || (lower.contains("cookie")
            && (lower.contains("permission denied")
                || lower.contains("operation not permitted")
                || lower.contains("locked")))
}

pub fn parse_manifest(json: &[u8], platform: Platform) -> Result<MediaManifest, ResolverError> {
    parse_manifests(json, platform)?
        .into_iter()
        .next()
        .ok_or(ResolverError::NoStreams)
}

pub fn parse_manifests(
    json: &[u8],
    platform: Platform,
) -> Result<Vec<MediaManifest>, ResolverError> {
    let raw: RawManifest = serde_json::from_slice(json)?;
    let inherited_headers = raw.http_headers.clone();
    if raw.kind.as_deref() == Some("playlist") || !raw.entries.is_empty() {
        let manifests = raw
            .entries
            .into_iter()
            .flatten()
            .filter_map(|entry| parse_raw_manifest(entry, platform, &inherited_headers).ok())
            .collect::<Vec<_>>();
        if manifests.is_empty() {
            return Err(ResolverError::NoStreams);
        }
        return Ok(manifests);
    }
    Ok(vec![parse_raw_manifest(raw, platform, &HashMap::new())?])
}

fn parse_raw_manifest(
    raw: RawManifest,
    platform: Platform,
    inherited_headers: &HashMap<String, String>,
) -> Result<MediaManifest, ResolverError> {
    let id = raw.id.ok_or(ResolverError::MissingField("id"))?;
    let title = raw.title.ok_or(ResolverError::MissingField("title"))?;
    let mut top_level_headers = inherited_headers.clone();
    top_level_headers.extend(raw.http_headers.clone());
    let formats: Vec<ParsedFormat> = raw
        .formats
        .into_iter()
        .enumerate()
        .filter_map(|(index, format)| ParsedFormat::from_raw(format, index, &top_level_headers))
        .collect();
    let streams = rank_streams(&formats);
    if streams.is_empty() {
        return Err(ResolverError::NoStreams);
    }
    Ok(MediaManifest {
        id,
        title,
        uploader: raw.uploader.unwrap_or_else(|| "Unknown".to_owned()),
        duration_secs: raw.duration,
        thumbnail_url: raw.thumbnail,
        platform,
        streams,
    })
}

impl ParsedFormat {
    fn from_raw(
        raw: RawFormat,
        index: usize,
        top_level_headers: &HashMap<String, String>,
    ) -> Option<Self> {
        let url = raw.url?;
        let container = normalize_container(&raw.ext.unwrap_or_else(|| "mp4".to_owned()))?;
        let resolution = raw.resolution.or_else(|| match (raw.width, raw.height) {
            (Some(width), Some(height)) => Some(format!("{width}x{height}")),
            (None, Some(height)) => Some(format!("{height}p")),
            _ => None,
        });
        let mut headers = top_level_headers.clone();
        headers.extend(raw.http_headers);
        Some(Self {
            format_id: raw.format_id.unwrap_or_else(|| format!("format-{index}")),
            url,
            protocol: raw.protocol.unwrap_or_default().to_ascii_lowercase(),
            container,
            resolution,
            fps: raw.fps,
            filesize_bytes: raw.filesize.or(raw.filesize_approx),
            vcodec: normalize_codec(raw.vcodec),
            acodec: normalize_codec(raw.acodec),
            abr: raw.abr,
            headers,
        })
    }
    fn is_hls(&self) -> bool {
        self.protocol == "m3u8" || self.protocol == "m3u8_native"
    }
    fn has_video(&self) -> bool {
        self.vcodec
            .as_deref()
            .map(|codec| codec != "none")
            .unwrap_or(false)
    }
    fn has_audio(&self) -> bool {
        self.acodec
            .as_deref()
            .map(|codec| codec != "none")
            .unwrap_or(false)
    }
    fn is_image(&self) -> bool {
        matches!(self.container.as_str(), "jpg" | "png")
    }
}

fn authentication_message(stderr: &str) -> Option<String> {
    let lower = stderr.to_ascii_lowercase();
    let patterns = [
        "login required",
        "sign in to confirm",
        "sign in to confirm your age",
        "sign in to confirm you're not a bot",
        "sign in to confirm you’re not a bot",
        "rate-limit reached",
        "instagram api is not granting access",
        "cookies",
        "cookie",
        "private video",
        "members-only",
        "join this channel",
        "bot verification",
        "http error 429",
        "http error 403",
    ];
    patterns
        .iter()
        .find(|pattern| lower.contains(**pattern))
        .map(|pattern| format!("{pattern}; enable a browser cookie source and retry"))
}

fn rank_streams(formats: &[ParsedFormat]) -> Vec<MediaStream> {
    let audio_formats: Vec<&ParsedFormat> = formats
        .iter()
        .filter(|format| format.has_audio() && !format.has_video())
        .collect();
    let best_audio = audio_formats
        .iter()
        .copied()
        .max_by(|left, right| audio_quality(left).cmp(&audio_quality(right)));
    let mut streams = Vec::new();
    let mut seen = HashSet::new();
    for format in formats {
        let stream = if format.is_hls() {
            Some(to_hls_stream(format))
        } else if format.is_image() || (format.has_video() && format.has_audio()) {
            Some(to_direct_stream(format))
        } else if format.has_video() {
            best_audio.map(|audio| to_split_stream(format, audio))
        } else if format.has_audio() {
            Some(to_direct_stream(format))
        } else {
            None
        };
        if let Some(stream) = stream {
            if seen.insert(stream_key(&stream)) {
                streams.push(stream);
            }
        }
    }
    streams.sort_by_key(|stream| std::cmp::Reverse(stream_quality(stream)));
    streams
}

fn to_direct_stream(format: &ParsedFormat) -> MediaStream {
    MediaStream {
        format_id: format.format_id.clone(),
        delivery: StreamDelivery::Direct {
            url: format.url.clone(),
        },
        container: format.container.clone(),
        resolution: format.resolution.clone(),
        fps: format.fps,
        filesize_bytes: format.filesize_bytes,
        vcodec: format.vcodec.clone(),
        acodec: format.acodec.clone(),
        headers: format.headers.clone(),
    }
}

fn to_hls_stream(format: &ParsedFormat) -> MediaStream {
    let mut stream = to_direct_stream(format);
    stream.delivery = StreamDelivery::Hls {
        manifest_url: format.url.clone(),
    };
    stream
}

fn to_split_stream(video: &ParsedFormat, audio: &ParsedFormat) -> MediaStream {
    let mut headers = video.headers.clone();
    for (key, value) in &audio.headers {
        headers.entry(key.clone()).or_insert_with(|| value.clone());
    }
    MediaStream {
        format_id: format!("{}+{}", video.format_id, audio.format_id),
        delivery: StreamDelivery::SplitDash {
            video_url: video.url.clone(),
            audio_url: audio.url.clone(),
        },
        container: video.container.clone(),
        resolution: video.resolution.clone(),
        fps: video.fps,
        filesize_bytes: combined_size(video.filesize_bytes, audio.filesize_bytes),
        vcodec: video.vcodec.clone(),
        acodec: audio.acodec.clone(),
        headers,
    }
}

fn normalize_container(extension: &str) -> Option<String> {
    match extension.to_ascii_lowercase().as_str() {
        "mp4" | "m4v" => Some("mp4".to_owned()),
        "webm" => Some("webm".to_owned()),
        "m4a" | "mp3" | "aac" | "opus" | "vorbis" => Some("m4a".to_owned()),
        "jpg" | "jpeg" => Some("jpg".to_owned()),
        "png" => Some("png".to_owned()),
        _ => None,
    }
}

fn normalize_codec(codec: Option<String>) -> Option<String> {
    match codec {
        Some(value) => {
            let lower = value.to_ascii_lowercase();
            if lower == "none" || lower.is_empty() {
                None
            } else {
                Some(lower)
            }
        }
        None => None,
    }
}

fn combined_size(video: Option<u64>, audio: Option<u64>) -> Option<u64> {
    match (video, audio) {
        (Some(video), Some(audio)) => video.checked_add(audio),
        (Some(size), None) | (None, Some(size)) => Some(size),
        (None, None) => None,
    }
}

fn audio_quality(format: &ParsedFormat) -> (u8, i64, i64) {
    let is_apple_compatible = u8::from(
        format.container == "m4a"
            || format
                .acodec
                .as_deref()
                .map(|c| c.contains("aac") || c.contains("mp4a"))
                .unwrap_or(false),
    );
    (
        is_apple_compatible,
        format.abr.unwrap_or_default() as i64,
        format.filesize_bytes.unwrap_or_default() as i64,
    )
}

fn stream_quality(stream: &MediaStream) -> (i64, u8, u8, i64, i64) {
    let resolution = stream
        .resolution
        .as_deref()
        .map(resolution_height)
        .unwrap_or_default() as i64;
    let fps = stream.fps.unwrap_or_default() as i64;
    let delivery_priority = match stream.delivery {
        StreamDelivery::SplitDash { .. } => 3,
        StreamDelivery::Direct { .. } => 2,
        StreamDelivery::Hls { .. } => 1,
    };
    let is_mp4 = u8::from(stream.container == "mp4" || stream.container == "m4a");
    let filesize = stream.filesize_bytes.unwrap_or_default() as i64;
    (resolution, delivery_priority, is_mp4, fps, filesize)
}

fn resolution_height(value: &str) -> u32 {
    let lower = value.to_ascii_lowercase();
    if lower.contains("4k") {
        return 2160;
    }
    lower
        .split('x')
        .next_back()
        .unwrap_or_default()
        .trim_end_matches('p')
        .parse()
        .unwrap_or_default()
}

fn stream_key(stream: &MediaStream) -> String {
    match &stream.delivery {
        StreamDelivery::Direct { url } => format!("direct:{url}"),
        StreamDelivery::SplitDash {
            video_url,
            audio_url,
        } => format!("split:{video_url}:{audio_url}"),
        StreamDelivery::Hls { manifest_url } => format!("hls:{manifest_url}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_pairs_video_with_preferred_m4a_audio() {
        let json = br#"{"id":"abc","title":"Example","duration":12.5,"http_headers":{"Cookie":"session=abc"},"formats":[{"format_id":"137","url":"https://cdn/video.mp4","ext":"mp4","height":1080,"vcodec":"avc1","acodec":"none","filesize":100,"http_headers":{"Referer":"https://example"}},{"format_id":"140","url":"https://cdn/audio.m4a","ext":"m4a","vcodec":"none","acodec":"aac","abr":128,"filesize_approx":20}]}"#;
        let result = parse_manifest(json, Platform::YouTube);
        assert!(result.is_ok());
        let manifest = match result {
            Ok(value) => value,
            Err(_) => return,
        };
        assert_eq!(manifest.duration_secs, Some(12.5));
        assert_eq!(manifest.streams.len(), 2);
        assert_eq!(manifest.streams[0].format_id, "137+140");
        assert_eq!(manifest.streams[0].filesize_bytes, Some(120));
        assert!(matches!(
            manifest.streams[0].delivery,
            StreamDelivery::SplitDash { .. }
        ));
        assert_eq!(
            manifest.streams[0].headers.get("Referer"),
            Some(&"https://example".to_owned())
        );
        assert_eq!(
            manifest.streams[0].headers.get("Cookie"),
            Some(&"session=abc".to_owned())
        );
    }

    #[test]
    fn maps_hls_and_ignores_unknown_containers() {
        let json = br#"{"id":"hls","title":"HLS","formats":[{"format_id":"hls","url":"https://cdn/master.m3u8","protocol":"m3u8_native","ext":"mp4","vcodec":"h264","acodec":"aac"},{"format_id":"bad","url":"https://cdn/subs.vtt","ext":"vtt","vcodec":"none","acodec":"none"}]}"#;
        let result = parse_manifest(json, Platform::Generic);
        assert!(result.is_ok());
        let manifest = match result {
            Ok(value) => value,
            Err(_) => return,
        };
        assert_eq!(manifest.streams.len(), 1);
        assert!(matches!(
            manifest.streams[0].delivery,
            StreamDelivery::Hls { .. }
        ));
    }

    #[test]
    fn classifies_authentication_stderr() {
        let message = authentication_message("ERROR: Sign in to confirm you are not a bot");
        assert!(message.is_some());
        assert!(message.unwrap_or_default().contains("browser cookie"));
        assert!(authentication_message("ERROR: network unavailable").is_none());
    }

    #[test]
    fn parses_playlist_entries_with_inherited_and_format_headers() {
        let json = br#"{"_type":"playlist","id":"carousel","title":"Carousel","http_headers":{"Cookie":"session=abc"},"entries":[{"id":"one","title":"Slide One","formats":[{"format_id":"one","url":"https://cdn/one.jpg","ext":"jpg","vcodec":"none","acodec":"none","http_headers":{"Referer":"https://instagram.com"}}]},{"id":"two","title":"Slide Two","formats":[{"format_id":"two","url":"https://cdn/two.jpg","ext":"jpg","vcodec":"none","acodec":"none"}]},{"id":"three","title":"Slide Three","formats":[{"format_id":"three","url":"https://cdn/three.jpg","ext":"jpg","vcodec":"none","acodec":"none"}]}]}"#;
        let result = parse_manifests(json, Platform::Instagram);
        assert!(result.is_ok());
        let manifests = match result {
            Ok(value) => value,
            Err(_) => return,
        };
        assert_eq!(manifests.len(), 3);
        assert_eq!(manifests[0].id, "one");
        assert_eq!(manifests[1].id, "two");
        assert_eq!(manifests[2].id, "three");
        assert_eq!(
            manifests[0].streams[0].headers.get("Cookie"),
            Some(&"session=abc".to_owned())
        );
        assert_eq!(
            manifests[0].streams[0].headers.get("Referer"),
            Some(&"https://instagram.com".to_owned())
        );
    }
}
