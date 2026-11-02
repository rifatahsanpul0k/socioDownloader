//! Low-latency native probing for direct media and Pinterest OpenGraph pages.

use std::collections::HashMap;

use reqwest::header::{HeaderMap, CONTENT_LENGTH, CONTENT_TYPE, RANGE};
use reqwest::{Client, Response};
use scraper::{Html, Selector};
use url::Url;

use super::sanitizer::{Platform, SanitizedUrl};
use super::types::{MediaManifest, MediaStream, StreamDelivery};
use super::ResolverError;

pub async fn try_resolve(
    client: &Client,
    sanitized: &SanitizedUrl,
) -> Result<Option<MediaManifest>, ResolverError> {
    if matches!(
        sanitized.platform,
        Platform::YouTube | Platform::Instagram | Platform::Facebook
    ) {
        return Ok(None);
    }
    if sanitized.platform == Platform::Pinterest && is_pinterest_pin(&sanitized.url) {
        return Ok(try_open_graph(client, sanitized).await);
    }

    let response = match probe_media(client, &sanitized.url).await {
        Ok(response) => response,
        Err(_) => return Ok(None),
    };
    let Some(content_type) = response_content_type(&response) else {
        return Ok(None);
    };
    if !is_direct_media_type(&content_type) {
        return Ok(None);
    }
    Ok(Some(direct_manifest(
        &sanitized.url,
        sanitized.platform,
        &content_type,
        response.headers(),
    )))
}

async fn probe_media(client: &Client, url: &Url) -> Result<Response, ResolverError> {
    if let Ok(response) = client.head(url.clone()).send().await {
        if response.status().is_success()
            && response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(is_direct_media_type)
                .unwrap_or(false)
        {
            return Ok(response);
        }
    }
    let response = client
        .get(url.clone())
        .header(RANGE, "bytes=0-0")
        .send()
        .await?;
    if response.status().is_success() || response.status().as_u16() == 206 {
        Ok(response)
    } else {
        Err(ResolverError::InvalidUrl(format!(
            "media probe returned HTTP {}",
            response.status()
        )))
    }
}

async fn try_open_graph(client: &Client, sanitized: &SanitizedUrl) -> Option<MediaManifest> {
    let response = client.get(sanitized.url.clone()).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let html = response.text().await.ok()?;
    let document = Html::parse_document(&html);
    let title = meta_content(&document, "og:title");
    let image_url = meta_content(&document, "og:image");
    let video_url = meta_content(&document, "og:video");
    let id = media_id(&sanitized.url);
    let title = title.unwrap_or_else(|| id.clone());

    if let Some(video_url) = video_url {
        if is_hls_url(&video_url) {
            return None;
        }
        return Some(MediaManifest {
            id,
            title,
            uploader: "Pinterest".to_owned(),
            duration_secs: None,
            thumbnail_url: image_url,
            platform: Platform::Pinterest,
            streams: vec![MediaStream {
                format_id: "og:video".to_owned(),
                delivery: StreamDelivery::Direct { url: video_url },
                container: "mp4".to_owned(),
                resolution: None,
                fps: None,
                filesize_bytes: None,
                vcodec: None,
                acodec: None,
                headers: HashMap::new(),
            }],
        });
    }

    let image_url = image_url?;
    Some(MediaManifest {
        id,
        title,
        uploader: "Pinterest".to_owned(),
        duration_secs: None,
        thumbnail_url: Some(image_url.clone()),
        platform: Platform::Pinterest,
        streams: vec![MediaStream {
            format_id: "og:image".to_owned(),
            delivery: StreamDelivery::Direct { url: image_url },
            container: "jpg".to_owned(),
            resolution: None,
            fps: None,
            filesize_bytes: None,
            vcodec: None,
            acodec: None,
            headers: HashMap::new(),
        }],
    })
}

fn meta_content(document: &Html, property: &str) -> Option<String> {
    let selector = Selector::parse(&format!(r#"meta[property="{property}"]"#)).ok()?;
    document
        .select(&selector)
        .find_map(|element| element.value().attr("content").map(str::to_owned))
}

fn direct_manifest(
    url: &Url,
    platform: Platform,
    content_type: &str,
    headers: &HeaderMap,
) -> MediaManifest {
    let id = media_id(url);
    let mut captured_headers = HashMap::new();
    for (name, value) in headers {
        if let Ok(value) = value.to_str() {
            captured_headers.insert(name.as_str().to_owned(), value.to_owned());
        }
    }
    let filesize_bytes = headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    MediaManifest {
        id: id.clone(),
        title: id,
        uploader: url.host_str().unwrap_or("unknown").to_owned(),
        duration_secs: None,
        thumbnail_url: if content_type.starts_with("image/") {
            Some(url.to_string())
        } else {
            None
        },
        platform,
        streams: vec![MediaStream {
            format_id: "native".to_owned(),
            delivery: if is_hls_url(url.as_str()) {
                StreamDelivery::Hls {
                    manifest_url: url.to_string(),
                }
            } else {
                StreamDelivery::Direct {
                    url: url.to_string(),
                }
            },
            container: media_container(url.as_str(), content_type),
            resolution: None,
            fps: None,
            filesize_bytes,
            vcodec: None,
            acodec: None,
            headers: captured_headers,
        }],
    }
}

fn response_content_type(response: &Response) -> Option<String> {
    response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
        })
}

fn is_direct_media_type(content_type: &str) -> bool {
    content_type.starts_with("image/")
        || content_type.starts_with("video/")
        || content_type.starts_with("audio/")
}

fn media_container(url: &str, content_type: &str) -> String {
    let extension = Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.path_segments()?.next_back().map(str::to_owned))
        .and_then(|segment| segment.split('.').next_back().map(str::to_owned))
        .map(|value| value.to_ascii_lowercase());
    match extension.as_deref() {
        Some("png") => "png",
        Some("webm") => "webm",
        Some("m4a") | Some("aac") | Some("mp3") | Some("ogg") => "m4a",
        Some("jpg") | Some("jpeg") => "jpg",
        Some("mp4") | Some("m4v") => "mp4",
        _ if content_type == "image/png" => "png",
        _ if content_type == "video/webm" || content_type == "audio/webm" => "webm",
        _ if content_type.starts_with("image/") => "jpg",
        _ if content_type.starts_with("audio/") => "m4a",
        _ => "mp4",
    }
    .to_owned()
}

fn is_hls_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.contains(".m3u8") || lower.contains("m3u8_native")
}

fn is_pinterest_pin(url: &Url) -> bool {
    url.host_str()
        .map(|host| {
            host.eq_ignore_ascii_case("pin.it")
                || (host.to_ascii_lowercase().ends_with("pinterest.com")
                    && url.path().contains("/pin/"))
        })
        .unwrap_or(false)
}

fn media_id(url: &Url) -> String {
    url.path_segments()
        .and_then(|mut segments| segments.rfind(|segment| !segment.is_empty()))
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| "direct-media".to_owned())
}
