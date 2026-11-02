//! URL resolution and normalized media metadata extraction.

mod native;
pub mod sanitizer;
pub mod types;
pub mod ytdlp;

use std::time::Duration;

use thiserror::Error;

use crate::auth::CookieSource;
use crate::sidecar::SidecarManager;
use sanitizer::sanitize_url;

pub use sanitizer::extract_batch_urls;
pub use sanitizer::Platform;
pub use types::{MediaManifest, MediaStream, StreamDelivery};

const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.6 Safari/605.1.15";

#[derive(Debug, Error)]
pub enum ResolverError {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("URL must use the http or https scheme")]
    UnsupportedScheme,
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("could not build the resolver HTTP client: {0}")]
    Client(String),
    #[error("could not read yt-dlp output: {0}")]
    ProcessIo(#[source] std::io::Error),
    #[error("yt-dlp exited unsuccessfully: {stderr}")]
    YtDlpFailed { stderr: String },
    #[error("authentication required for {platform}: {message}")]
    AuthenticationRequired { platform: Platform, message: String },
    #[error("could not parse yt-dlp JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("yt-dlp returned no usable media streams")]
    NoStreams,
    #[error("yt-dlp JSON is missing the required field '{0}'")]
    MissingField(&'static str),
}

#[derive(Clone)]
pub struct Resolver {
    client: reqwest::Client,
    sidecar: SidecarManager,
    cookie_source: CookieSource,
    max_playlist_items: usize,
}

impl Resolver {
    pub fn new(sidecar: SidecarManager) -> Result<Self, ResolverError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::limited(10))
            .user_agent(BROWSER_USER_AGENT)
            .build()
            .map_err(|error| ResolverError::Client(error.to_string()))?;
        Ok(Self {
            client,
            sidecar,
            cookie_source: CookieSource::None,
            max_playlist_items: 25,
        })
    }

    pub async fn resolve(&self, input: &str) -> Result<MediaManifest, ResolverError> {
        self.resolve_with_cookies(input, Some(&self.cookie_source))
            .await
    }

    pub fn with_cookie_source(mut self, cookie_source: CookieSource) -> Self {
        self.cookie_source = cookie_source;
        self
    }

    pub fn with_max_playlist_items(mut self, max_playlist_items: usize) -> Self {
        self.max_playlist_items = max_playlist_items.max(1);
        self
    }

    pub async fn resolve_with_cookies(
        &self,
        input: &str,
        cookie_source: Option<&CookieSource>,
    ) -> Result<MediaManifest, ResolverError> {
        let sanitized = sanitize_url(input)?;
        match tokio::time::timeout(
            Duration::from_millis(100),
            native::try_resolve(&self.client, &sanitized),
        )
        .await
        {
            Ok(Ok(Some(manifest))) => Ok(manifest),
            Ok(Ok(None)) | Ok(Err(_)) => {
                ytdlp::resolve(&self.sidecar, &sanitized, cookie_source).await
            }
            Err(_) => match native::try_resolve(&self.client, &sanitized).await {
                Ok(Some(manifest)) => Ok(manifest),
                Ok(None) | Err(_) => ytdlp::resolve(&self.sidecar, &sanitized, cookie_source).await,
            },
        }
    }

    pub async fn resolve_all(
        &self,
        input: &str,
        cookies: &CookieSource,
        allow_playlist: bool,
    ) -> Result<Vec<MediaManifest>, crate::CoreError> {
        let sanitized = sanitize_url(input)?;
        if should_expand_playlist(&sanitized, allow_playlist) {
            return ytdlp::resolve_all(
                &self.sidecar,
                &sanitized,
                Some(cookies),
                self.max_playlist_items,
            )
            .await
            .map_err(Into::into);
        }
        self.resolve_with_cookies(sanitized.url.as_ref(), Some(cookies))
            .await
            .map(|manifest| vec![manifest])
            .map_err(Into::into)
    }
}

fn should_expand_playlist(sanitized: &sanitizer::SanitizedUrl, allow_playlist: bool) -> bool {
    let path = sanitized.url.path();
    match sanitized.platform {
        Platform::Instagram => path.split('/').any(|segment| segment == "p"),
        Platform::YouTube => {
            let explicit_playlist = path == "/playlist"
                && sanitized
                    .url
                    .query_pairs()
                    .any(|(key, value)| key == "list" && !value.is_empty());
            explicit_playlist || allow_playlist
        }
        _ => allow_playlist,
    }
}
