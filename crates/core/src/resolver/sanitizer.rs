//! URL validation, tracking-parameter removal, and platform classification.

use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};
use url::Url;

use super::ResolverError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    YouTube,
    Instagram,
    Facebook,
    Pinterest,
    Generic,
}

impl fmt::Display for Platform {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::YouTube => "YouTube",
            Self::Instagram => "Instagram",
            Self::Facebook => "Facebook",
            Self::Pinterest => "Pinterest",
            Self::Generic => "Generic",
        })
    }
}

#[derive(Debug, Clone)]
pub struct SanitizedUrl {
    pub url: Url,
    pub platform: Platform,
}

pub fn sanitize_url(input: &str) -> Result<SanitizedUrl, ResolverError> {
    let mut url =
        Url::parse(input.trim()).map_err(|error| ResolverError::InvalidUrl(error.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ResolverError::UnsupportedScheme);
    }
    if url.host_str().is_none() {
        return Err(ResolverError::InvalidUrl("URL has no host".to_owned()));
    }
    let retained: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| !is_tracking_parameter(key))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    url.set_query(None);
    if !retained.is_empty() {
        let mut query = url.query_pairs_mut();
        for (key, value) in retained {
            query.append_pair(&key, &value);
        }
    }
    let platform = classify_host(url.host_str().unwrap_or_default());
    Ok(SanitizedUrl { url, platform })
}

/// Extracts valid, sanitized URLs from pasted batch input while preserving order.
pub fn extract_batch_urls(input: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    input
        .split(|character: char| character.is_whitespace() || character == ',')
        .filter_map(|candidate| sanitize_url(candidate).ok())
        .map(|sanitized| sanitized.url.to_string())
        .filter(|url| seen.insert(url.clone()))
        .collect()
}

fn is_tracking_parameter(key: &str) -> bool {
    key.starts_with("utm_")
        || matches!(
            key.to_ascii_lowercase().as_str(),
            "igsh" | "igshid" | "fbclid" | "si" | "feature"
        )
}

pub fn classify_host(host: &str) -> Platform {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host == "youtu.be" || is_domain_or_subdomain(&host, "youtube.com") {
        Platform::YouTube
    } else if is_domain_or_subdomain(&host, "instagram.com") {
        Platform::Instagram
    } else if host == "fb.watch" || is_domain_or_subdomain(&host, "facebook.com") {
        Platform::Facebook
    } else if host == "pin.it"
        || is_domain_or_subdomain(&host, "pinterest.com")
        || is_domain_or_subdomain(&host, "pinimg.com")
    {
        Platform::Pinterest
    } else {
        Platform::Generic
    }
}

fn is_domain_or_subdomain(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_tracking_and_preserves_media_parameters() {
        let result = sanitize_url(
            "https://www.youtube.com/watch?v=abc&utm_source=x&list=PL1&si=x&t=42&feature=share",
        );
        assert!(result.is_ok());
        let sanitized = match result {
            Ok(value) => value,
            Err(_) => return,
        };
        assert_eq!(sanitized.platform, Platform::YouTube);
        assert_eq!(sanitized.url.query(), Some("v=abc&list=PL1&t=42"));
    }

    #[test]
    fn classifies_platforms_and_rejects_bad_schemes() {
        assert_eq!(classify_host("m.youtube.com"), Platform::YouTube);
        assert_eq!(classify_host("instagram.com"), Platform::Instagram);
        assert_eq!(classify_host("fb.watch"), Platform::Facebook);
        assert_eq!(classify_host("pin.it"), Platform::Pinterest);
        assert_eq!(classify_host("cdn.example.test"), Platform::Generic);
        assert!(matches!(
            sanitize_url("file:///tmp/a"),
            Err(ResolverError::UnsupportedScheme)
        ));
    }

    #[test]
    fn extracts_and_deduplicates_messy_batch_input() {
        let input = "https://youtu.be/abc, https://youtu.be/abc?si=tracking\n\nhttps://www.instagram.com/p/slide/?utm_source=share https://example.test/media.mp4";
        let urls = extract_batch_urls(input);
        assert_eq!(
            urls,
            vec![
                "https://youtu.be/abc".to_owned(),
                "https://www.instagram.com/p/slide/".to_owned(),
                "https://example.test/media.mp4".to_owned(),
            ]
        );
    }
}
