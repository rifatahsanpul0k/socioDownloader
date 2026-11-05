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
    GitHub,
    Generic,
}

impl fmt::Display for Platform {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::YouTube => "YouTube",
            Self::Instagram => "Instagram",
            Self::Facebook => "Facebook",
            Self::Pinterest => "Pinterest",
            Self::GitHub => "GitHub",
            Self::Generic => "Generic",
        })
    }
}

#[derive(Debug, Clone)]
pub struct SanitizedUrl {
    pub url: Url,
    pub platform: Platform,
}

/// Scans text for any embedded http:// or https:// URL, stripping surrounding text, emojis, or punctuation.
pub fn find_url_in_text(text: &str) -> Option<&str> {
    let prefixes = ["https://", "http://"];
    for prefix in prefixes {
        if let Some(start_idx) = text.find(prefix) {
            let candidate = &text[start_idx..];
            let end_idx = candidate
                .char_indices()
                .find_map(|(idx, ch)| {
                    if ch.is_whitespace() || matches!(ch, '"' | '\'' | '<' | '>' | '`') {
                        Some(idx)
                    } else {
                        None
                    }
                })
                .unwrap_or(candidate.len());
            let mut url_str = &candidate[..end_idx];
            while url_str.ends_with(|c: char| {
                matches!(
                    c,
                    '.' | ',' | '!' | '?' | ';' | ':' | ')' | ']' | '}' | '>' | '"' | '\'' | ' '
                )
            }) {
                url_str = &url_str[..url_str.len() - 1];
            }
            if !url_str.is_empty() {
                return Some(url_str);
            }
        }
    }
    None
}

pub fn sanitize_url(input: &str) -> Result<SanitizedUrl, ResolverError> {
    let trimmed = input.trim();
    let parsed_url = Url::parse(trimmed)
        .or_else(|_| {
            if let Some(extracted) = find_url_in_text(trimmed) {
                Url::parse(extracted)
            } else {
                Err(url::ParseError::RelativeUrlWithoutBase)
            }
        })
        .map_err(|error| ResolverError::InvalidUrl(error.to_string()))?;

    let mut url = parsed_url;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ResolverError::UnsupportedScheme);
    }
    let host = match url.host_str() {
        Some(h) => h.to_ascii_lowercase(),
        None => return Err(ResolverError::InvalidUrl("URL has no host".to_owned())),
    };

    let platform = classify_host(&host);

    // Preserve presigned S3 query parameters on githubusercontent.com or amazonaws.com
    let preserve_query = host.ends_with("githubusercontent.com")
        || host.ends_with("amazonaws.com")
        || host.contains("s3")
        || url
            .query()
            .map(|q| q.contains("X-Amz-Signature") || q.contains("response-content-disposition"))
            .unwrap_or(false);

    if !preserve_query {
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
    }

    // GitHub URL Rewrites:
    if platform == Platform::GitHub && host == "github.com" {
        let segments: Vec<&str> = url
            .path_segments()
            .map(|s| s.collect())
            .unwrap_or_default();
        // 1. Rewrite /blob/ URLs:
        // https://github.com/<owner>/<repo>/blob/<branch>/<path...>
        // -> https://raw.githubusercontent.com/<owner>/<repo>/<branch>/<path...>
        if segments.len() >= 4 && segments[2] == "blob" {
            let owner = segments[0];
            let repo = segments[1];
            let branch = segments[3];
            let rest = segments[4..].join("/");
            let new_url_str = if rest.is_empty() {
                format!("https://raw.githubusercontent.com/{owner}/{repo}/{branch}")
            } else {
                format!("https://raw.githubusercontent.com/{owner}/{repo}/{branch}/{rest}")
            };
            if let Ok(new_url) = Url::parse(&new_url_str) {
                url = new_url;
            }
        } else if (segments.len() == 2 || (segments.len() == 3 && segments[2].is_empty()))
            && !segments[0].is_empty()
            && !segments[1].is_empty()
            && !matches!(
                segments[0],
                "features" | "pricing" | "login" | "join" | "topics" | "trending" | "settings" | "orgs"
            )
        {
            // 2. Rewrite bare repo URLs:
            // https://github.com/<owner>/<repo>
            // -> https://codeload.github.com/<owner>/<repo>/zip/refs/heads/main
            let owner = segments[0];
            let repo = segments[1].trim_end_matches(".git");
            let new_url_str =
                format!("https://codeload.github.com/{owner}/{repo}/zip/refs/heads/main");
            if let Ok(new_url) = Url::parse(&new_url_str) {
                url = new_url;
            }
        }
    }

    let final_platform = classify_host(url.host_str().unwrap_or_default());
    Ok(SanitizedUrl {
        url,
        platform: final_platform,
    })
}

/// Extracts valid, sanitized URLs from pasted batch input while preserving order.
pub fn extract_batch_urls(input: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut urls = Vec::new();
    for token in input.split(|character: char| character.is_whitespace() || character == ',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        if let Ok(sanitized) = sanitize_url(token) {
            let u = sanitized.url.to_string();
            if seen.insert(u.clone()) {
                urls.push(u);
            }
        }
    }
    if urls.is_empty() {
        if let Some(candidate) = find_url_in_text(input) {
            if let Ok(sanitized) = sanitize_url(candidate) {
                let u = sanitized.url.to_string();
                if seen.insert(u.clone()) {
                    urls.push(u);
                }
            }
        }
    }
    urls
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
    } else if is_domain_or_subdomain(&host, "github.com")
        || is_domain_or_subdomain(&host, "githubusercontent.com")
    {
        Platform::GitHub
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

    #[test]
    fn extracts_url_from_messy_social_captions() {
        let input = "Check this out! https://youtu.be/abc123 😍";
        let sanitized = sanitize_url(input).expect("must extract embedded URL");
        assert_eq!(sanitized.platform, Platform::YouTube);
        assert_eq!(sanitized.url.as_str(), "https://youtu.be/abc123");
    }

    #[test]
    fn rewrites_github_blob_and_bare_repo_urls() {
        let blob_input = "https://github.com/rust-lang/rust/blob/master/README.md";
        let blob_sanitized = sanitize_url(blob_input).expect("must parse blob URL");
        assert_eq!(blob_sanitized.platform, Platform::GitHub);
        assert_eq!(
            blob_sanitized.url.as_str(),
            "https://raw.githubusercontent.com/rust-lang/rust/master/README.md"
        );

        let repo_input = "https://github.com/rust-lang/rust";
        let repo_sanitized = sanitize_url(repo_input).expect("must parse repo URL");
        assert_eq!(repo_sanitized.platform, Platform::GitHub);
        assert_eq!(
            repo_sanitized.url.as_str(),
            "https://codeload.github.com/rust-lang/rust/zip/refs/heads/main"
        );
    }

    #[test]
    fn preserves_presigned_s3_signatures() {
        let s3_url = "https://objects.githubusercontent.com/asset?X-Amz-Signature=sig123&X-Amz-Credential=cred456&response-content-disposition=attachment%3B%20filename%3Dapp.dmg";
        let sanitized = sanitize_url(s3_url).expect("must parse S3 URL");
        assert!(sanitized.url.query().unwrap().contains("X-Amz-Signature=sig123"));
        assert!(sanitized.url.query().unwrap().contains("response-content-disposition"));
    }
}
