//! Stable media types shared by native and yt-dlp resolution paths.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::Platform;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaManifest {
    pub id: String,
    pub title: String,
    pub uploader: String,
    pub duration_secs: Option<f64>,
    pub thumbnail_url: Option<String>,
    pub platform: Platform,
    pub streams: Vec<MediaStream>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaStream {
    pub format_id: String,
    pub delivery: StreamDelivery,
    pub container: String,
    pub resolution: Option<String>,
    pub fps: Option<f32>,
    pub filesize_bytes: Option<u64>,
    pub vcodec: Option<String>,
    pub acodec: Option<String>,
    pub headers: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StreamDelivery {
    Direct {
        url: String,
    },
    SplitDash {
        video_url: String,
        audio_url: String,
    },
    Hls {
        manifest_url: String,
    },
}
