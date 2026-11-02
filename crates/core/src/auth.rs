//! Browser-cookie sources understood by yt-dlp.

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowserKind {
    Safari,
    Chrome,
    Arc,
    Brave,
    Firefox,
    Edge,
}

impl fmt::Display for BrowserKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Safari => "safari",
            Self::Chrome => "chrome",
            Self::Arc => "arc",
            Self::Brave => "brave",
            Self::Firefox => "firefox",
            Self::Edge => "edge",
        })
    }
}

impl FromStr for BrowserKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "safari" => Ok(Self::Safari),
            "chrome" | "google-chrome" => Ok(Self::Chrome),
            "arc" => Ok(Self::Arc),
            "brave" => Ok(Self::Brave),
            "firefox" => Ok(Self::Firefox),
            "edge" | "microsoft-edge" => Ok(Self::Edge),
            other => Err(format!("unsupported browser '{other}'")),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CookieSource {
    #[default]
    None,
    Browser(BrowserKind),
    CookieFile(PathBuf),
}

impl CookieSource {
    pub fn to_ytdlp_args(&self) -> Vec<String> {
        match self {
            Self::None => Vec::new(),
            Self::Browser(BrowserKind::Arc) => {
                let profile = dirs::home_dir()
                    .map(|home| home.join("Library/Application Support/Arc/User Data"));
                let value = profile
                    .map(|path| format!("chrome:{}", path.display()))
                    .unwrap_or_else(|| {
                        "chrome:~/Library/Application Support/Arc/User Data".to_owned()
                    });
                vec!["--cookies-from-browser".to_owned(), value]
            }
            Self::Browser(browser) => {
                vec!["--cookies-from-browser".to_owned(), browser.to_string()]
            }
            Self::CookieFile(path) => {
                vec!["--cookies".to_owned(), path.to_string_lossy().into_owned()]
            }
        }
    }
}

pub fn detect_installed_browsers() -> Vec<BrowserKind> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let candidates = [
        (BrowserKind::Safari, vec![home.join("Library/Safari")]),
        (
            BrowserKind::Chrome,
            vec![home.join("Library/Application Support/Google/Chrome")],
        ),
        (
            BrowserKind::Arc,
            vec![home.join("Library/Application Support/Arc/User Data")],
        ),
        (
            BrowserKind::Brave,
            vec![home.join("Library/Application Support/BraveSoftware/Brave-Browser")],
        ),
        (
            BrowserKind::Firefox,
            vec![home.join("Library/Application Support/Firefox/Profiles")],
        ),
        (
            BrowserKind::Edge,
            vec![home.join("Library/Application Support/Microsoft Edge")],
        ),
    ];
    candidates
        .into_iter()
        .filter_map(|(browser, paths)| paths.iter().any(|path| path.exists()).then_some(browser))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_browser_and_file_arguments() {
        assert_eq!(
            CookieSource::Browser(BrowserKind::Safari).to_ytdlp_args(),
            vec!["--cookies-from-browser", "safari"]
        );
        assert_eq!(
            CookieSource::CookieFile(PathBuf::from("/tmp/cookies.txt")).to_ytdlp_args(),
            vec!["--cookies", "/tmp/cookies.txt"]
        );
    }

    #[test]
    fn builds_arc_chrome_profile_argument() {
        let args = CookieSource::Browser(BrowserKind::Arc).to_ytdlp_args();
        assert_eq!(
            args.first().map(String::as_str),
            Some("--cookies-from-browser")
        );
        assert!(args
            .get(1)
            .map(|value| value.starts_with("chrome:"))
            .unwrap_or(false));
        assert!(args
            .get(1)
            .map(|value| value.contains("Library/Application Support/Arc/User Data"))
            .unwrap_or(false));
    }
}
