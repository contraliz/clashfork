use crate::models::DiagnosticEntry;
use chrono::Utc;
use parking_lot::Mutex;
use regex::Regex;
use std::collections::VecDeque;
use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
};

const MAX_ENTRIES: usize = 400;

#[derive(Debug)]
pub struct Diagnostics {
    next_id: AtomicU64,
    entries: Mutex<VecDeque<DiagnosticEntry>>,
}

impl Default for Diagnostics {
    fn default() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            entries: Mutex::new(VecDeque::with_capacity(MAX_ENTRIES)),
        }
    }
}

impl Diagnostics {
    pub fn push(&self, source: &str, level: &str, message: impl AsRef<str>) {
        self.push_at(Utc::now().timestamp(), source, level, message);
    }

    fn push_at(&self, at: i64, source: &str, level: &str, message: impl AsRef<str>) {
        let message = truncate(&redact(message.as_ref()), 2_000);
        if message.trim().is_empty() {
            return;
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let entry = DiagnosticEntry {
            id,
            at,
            source: source.to_owned(),
            level: level.to_owned(),
            message,
        };
        let mut entries = self.entries.lock();
        while entries.len() >= MAX_ENTRIES {
            entries.pop_front();
        }
        entries.push_back(entry);
    }

    /// Newest first. IDs are assigned monotonically, so equal-second entries still have a
    /// deterministic, stable order at the snapshot boundary.
    pub fn snapshot(&self) -> Vec<DiagnosticEntry> {
        let mut entries: Vec<_> = self.entries.lock().iter().cloned().collect();
        entries.sort_by(|left, right| right.at.cmp(&left.at).then_with(|| right.id.cmp(&left.id)));
        entries
    }
}

fn truncate(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_owned();
    }
    let mut end = max;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

fn url_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r#"(?i)\b[a-z][a-z0-9+.-]*://[^\s"'<>]+"#).expect("valid URL redaction regex")
    })
}

fn query_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?i)\b(token|secret|password|passwd|uuid|key|auth|signature|sub|subscription)=([^&\s]+)")
            .expect("valid query redaction regex")
    })
}

fn assignment_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?i)\b(password|passwd|uuid|secret|token|authorization|credential|private-key|public-key|short-id)\b(\s*[:=]\s*)([^\s,;]+)")
            .expect("valid assignment redaction regex")
    })
}

pub fn redact(value: &str) -> String {
    let without_urls = url_regex().replace_all(value, "[redacted-url]");
    let without_queries = query_regex().replace_all(&without_urls, "$1=[redacted]");
    assignment_regex()
        .replace_all(&without_queries, "$1$2[redacted]")
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_urls_path_tokens_and_credentials() {
        let raw = "fetch https://user:pass@example.com/sub/topsecret?token=abc123&name=x failed; password=hunter2 uuid: 1234";
        let redacted = redact(raw);
        assert!(!redacted.contains("user:pass"));
        assert!(!redacted.contains("topsecret"));
        assert!(!redacted.contains("abc123"));
        assert!(!redacted.contains("hunter2"));
        assert!(!redacted.contains("1234"));
        assert!(redacted.contains("[redacted-url]"));
    }

    #[test]
    fn ring_is_bounded() {
        let diagnostics = Diagnostics::default();
        for index in 0..(MAX_ENTRIES + 20) {
            diagnostics.push("app", "info", format!("message {index}"));
        }
        let entries = diagnostics.snapshot();
        assert_eq!(entries.len(), MAX_ENTRIES);
        assert_eq!(
            entries.first().map(|entry| entry.message.as_str()),
            Some("message 419")
        );
        assert_eq!(
            entries.last().map(|entry| entry.message.as_str()),
            Some("message 20")
        );
    }

    #[test]
    fn snapshot_is_newest_first_with_monotonic_tie_break() {
        let diagnostics = Diagnostics::default();
        diagnostics.push_at(100, "app", "info", "first at same second");
        diagnostics.push_at(100, "core", "info", "second at same second");
        diagnostics.push_at(101, "service", "warn", "newer second");

        let entries = diagnostics.snapshot();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.message.as_str())
                .collect::<Vec<_>>(),
            vec![
                "newer second",
                "second at same second",
                "first at same second"
            ]
        );
        assert_eq!(entries[1].at, entries[2].at);
        assert!(entries[1].id > entries[2].id);
    }
}
