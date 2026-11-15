//! Hand-written formatting helpers (no extra dependencies).

use std::time::{Duration, SystemTime};

/// 1024-based units, one decimal: "612.3 MB". Bytes are shown whole.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = "B";
    for u in UNITS {
        value /= 1024.0;
        unit = u;
        if value < 1024.0 {
            break;
        }
    }
    format!("{value:.1} {unit}")
}

/// "today", "1 day ago", "N days ago", or "unknown".
pub fn days_ago(t: Option<SystemTime>, now: SystemTime) -> String {
    let Some(t) = t else {
        return "unknown".to_owned();
    };
    let days = now.duration_since(t).unwrap_or(Duration::ZERO).as_secs() / 86_400;
    match days {
        0 => "today".to_owned(),
        1 => "1 day ago".to_owned(),
        n => format!("{n} days ago"),
    }
}

/// Keeps the tail of a path (the interesting part) and marks the cut with "...".
pub fn shorten(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_owned();
    }
    let tail: String = s.chars().skip(count - (max - 3)).collect();
    format!("...{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(1023), "1023 B");
        assert_eq!(size(1024), "1.0 KB");
        assert_eq!(size(642_000_000), "612.3 MB");
        assert_eq!(size(1024 * 1024 * 1024), "1.0 GB");
    }

    #[test]
    fn days() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10 * 86_400);
        let ago = |d: u64| Some(now - Duration::from_secs(d * 86_400 + 5));
        assert_eq!(days_ago(None, now), "unknown");
        assert_eq!(days_ago(Some(now), now), "today");
        assert_eq!(days_ago(ago(1), now), "1 day ago");
        assert_eq!(days_ago(ago(5), now), "5 days ago");
    }

    #[test]
    fn shortening() {
        assert_eq!(shorten("short", 60), "short");
        let s = shorten(&"a".repeat(100), 60);
        assert_eq!(s.chars().count(), 60);
        assert!(s.starts_with("..."));
    }
}
