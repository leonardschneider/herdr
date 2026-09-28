//! On-demand local file candidates. Never called from rendering.
use crate::api::schema::{PaneFileKind, PaneFileTarget};
use std::{
    collections::VecDeque,
    fs::Metadata,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub(crate) struct ResolvedFile {
    pub path: PathBuf,
    pub metadata: Metadata,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub kind: PaneFileKind,
}

pub(crate) struct CachedTarget {
    pub pane_id: String,
    pub terminal_id: crate::terminal::TerminalId,
    pub child_pid: u32,
    pub file: ResolvedFile,
    created: Instant,
    token: String,
}

impl CachedTarget {
    pub(crate) fn matches_runtime(
        &self,
        terminal_id: &crate::terminal::TerminalId,
        child_pid: u32,
    ) -> bool {
        self.terminal_id == *terminal_id && self.child_pid == child_pid
    }
}

pub(crate) struct TargetCache {
    entries: VecDeque<CachedTarget>,
    incarnation: String,
    next: u64,
}

impl Default for TargetCache {
    fn default() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NONCE: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Self {
            entries: VecDeque::new(),
            incarnation: format!(
                "{}-{nanos}-{}",
                std::process::id(),
                NONCE.fetch_add(1, Ordering::Relaxed)
            ),
            next: 0,
        }
    }
}

impl TargetCache {
    pub fn insert(
        &mut self,
        pane_id: String,
        terminal_id: crate::terminal::TerminalId,
        child_pid: u32,
        file: ResolvedFile,
    ) -> Option<PaneFileTarget> {
        let path = file.path.to_str()?.to_owned();
        if path.chars().any(char::is_control) {
            return None;
        }
        self.entries
            .retain(|entry| entry.created.elapsed() < Duration::from_secs(60));
        while self.entries.len() >= 64 {
            self.entries.pop_front();
        }
        self.next = self.next.wrapping_add(1);
        let token = format!("file-{}-{}", self.incarnation, self.next);
        let target = PaneFileTarget {
            target_id: token.clone(),
            path,
            line: file.line,
            column: file.column,
            kind: file.kind,
        };
        self.entries.push_back(CachedTarget {
            pane_id,
            terminal_id,
            child_pid,
            file,
            created: Instant::now(),
            token,
        });
        Some(target)
    }

    pub fn take(&mut self, pane_id: &str, token: &str) -> Option<CachedTarget> {
        self.entries
            .retain(|entry| entry.created.elapsed() < Duration::from_secs(60));
        let index = self
            .entries
            .iter()
            .position(|entry| entry.token == token && entry.pane_id == pane_id)?;
        self.entries.remove(index)
    }
}

pub(crate) fn classify(sample: &[u8], file_len: u64) -> PaneFileKind {
    if sample.contains(&0) {
        return PaneFileKind::Binary;
    }
    match std::str::from_utf8(sample) {
        Ok(_) => PaneFileKind::Text,
        // Only an incomplete trailing codepoint at a bounded sample boundary is
        // tolerated; complete invalid sequences and incomplete whole files are not.
        Err(err) if err.error_len().is_none() && file_len > sample.len() as u64 => {
            PaneFileKind::Text
        }
        Err(_) => PaneFileKind::Unknown,
    }
}

pub(crate) fn remote_program(name: &str) -> bool {
    matches!(
        Path::new(name)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(name),
        "ssh"
            | "mosh"
            | "mosh-client"
            | "telnet"
            | "docker"
            | "podman"
            | "kubectl"
            | "nsenter"
            | "lxc"
            | "machinectl"
            | "tmux"
            | "screen"
    )
}

fn decode_file_uri(value: &str) -> Option<String> {
    let rest = value.strip_prefix("file://")?;
    // Query/fragment syntax is not a filename. Encoded punctuation remains valid.
    if rest.contains(['?', '#']) {
        return None;
    }
    let slash = rest.find('/')?;
    let host = &rest[..slash];
    if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
        return None;
    }
    let bytes = &rest.as_bytes()[slash..];
    let mut decoded = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn candidate_path(value: &str, cwd: &Path, home: Option<&Path>) -> Option<PathBuf> {
    if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return None;
    }
    let decoded;
    let value = if value.starts_with("file:") {
        decoded = decode_file_uri(value)?;
        decoded.as_str()
    } else {
        if value.contains("://") {
            return None;
        }
        value
    };
    if value.chars().any(char::is_control) {
        return None;
    }
    if let Some(relative) = value.strip_prefix("~/") {
        return Some(home?.join(relative));
    }
    if value.starts_with('~') {
        return None;
    }
    let path = Path::new(value);
    Some(if path.is_absolute() {
        path.to_owned()
    } else {
        cwd.join(path)
    })
}

fn probe(
    value: &str,
    cwd: &Path,
    home: Option<&Path>,
    line: Option<u32>,
    column: Option<u32>,
) -> Option<ResolvedFile> {
    let path = candidate_path(value, cwd, home)?.canonicalize().ok()?;
    if path.to_str()?.chars().any(char::is_control) {
        return None;
    }
    let (metadata, kind) = match crate::platform::sample_local_regular_file(&path) {
        Ok((metadata, sample)) => {
            let kind = classify(&sample, metadata.len());
            (metadata, kind)
        }
        Err(_) => (std::fs::metadata(&path).ok()?, PaneFileKind::Unknown),
    };
    if !metadata.is_file() {
        return None;
    }
    Some(ResolvedFile {
        path,
        metadata,
        line,
        column,
        kind,
    })
}

fn cursor_number(value: &str) -> Option<u32> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value
        .parse::<u32>()
        .ok()
        .filter(|n| *n > 0 && *n <= i32::MAX as u32)
}

/// Exact existing filenames win over diagnostic line/column suffixes.
pub(crate) fn resolve_candidate(
    value: &str,
    cwd: &Path,
    home: Option<&Path>,
) -> Option<ResolvedFile> {
    if let Some(file) = probe(value, cwd, home, None, None) {
        return Some(file);
    }
    if let Some(quote @ ('\'' | '"' | '`')) = value.chars().next() {
        let closing = value[1..].find(quote)? + 1;
        let path = &value[1..closing];
        let suffix = &value[closing + 1..];
        if suffix.is_empty() {
            return resolve_candidate(path, cwd, home);
        }
        let mut parts = suffix.strip_prefix(':')?.split(':');
        let line = cursor_number(parts.next()?)?;
        let column = match parts.next() {
            Some(part) => Some(cursor_number(part)?),
            None => None,
        };
        if parts.next().is_some() {
            return None;
        }
        return probe(path, cwd, home, Some(line), column);
    }
    let (prefix, last) = value.rsplit_once(':')?;
    let last = cursor_number(last)?;
    if let Some((path, line)) = prefix.rsplit_once(':') {
        if !line.is_empty() && line.bytes().all(|b| b.is_ascii_digit()) {
            let line = cursor_number(line)?;
            if let Some(file) = probe(path, cwd, home, Some(line), Some(last)) {
                return Some(file);
            }
        }
    }
    probe(prefix, cwd, home, Some(last), None)
}

/// Return a bounded set, preferring quoted paths then whitespace-delimited text.
pub(crate) fn candidates_at(text: &str, byte: usize) -> Vec<String> {
    if byte >= text.len() || !text.is_char_boundary(byte) {
        return Vec::new();
    }
    let start = text[..byte].rfind('\n').map_or(0, |n| n + 1);
    let end = text[byte..].find('\n').map_or(text.len(), |n| byte + n);
    let row = &text[start..end];
    if row.len() > 16384 {
        return Vec::new();
    }
    let clicked = byte - start;
    let mut result = Vec::new();
    // A single active delimiter preserves other quote characters literally.
    let mut open = None;
    for (i, ch) in row.char_indices() {
        if let Some((begin, quote)) = open {
            if ch != quote {
                continue;
            }
            let suffix_end = row[i + 1..]
                .find(|c: char| c != ':' && !c.is_ascii_digit())
                .map_or(row.len(), |n| i + 1 + n);
            if clicked >= begin && clicked < suffix_end {
                return vec![row[begin..suffix_end].to_owned()];
            }
            open = None;
        } else if matches!(ch, '\'' | '"' | '`')
            && (i == 0
                || row[..i]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_whitespace() || matches!(c, '(' | '[' | '{' | '=')))
        {
            open = Some((i, ch));
        }
    }
    let left = row[..clicked]
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(i, c)| i + c.len_utf8());
    let right = row[clicked..]
        .find(char::is_whitespace)
        .map_or(row.len(), |i| clicked + i);
    let raw = &row[left..right];
    result.push(raw.to_owned());
    let trimmed = raw.trim_matches(|c| {
        matches!(
            c,
            '\'' | '"' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';'
        )
    });
    if trimmed != raw {
        result.push(trimmed.to_owned());
    }
    result.truncate(4);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    #[test]
    fn file_resolution_cache_and_identity() {
        let dir = std::env::temp_dir().join(format!(
            "herdr-file-test-{}",
            crate::terminal::TerminalId::alloc()
        ));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("a b;$(false).rs");
        std::fs::write(&path, "hello").unwrap();
        let file = resolve_candidate("a b;$(false).rs:12:3", &dir, None).unwrap();
        assert_eq!(
            (file.line, file.column, file.kind),
            (Some(12), Some(3), PaneFileKind::Text)
        );
        let mut cache = TargetCache::default();
        let target = cache
            .insert(
                "pane_1".into(),
                crate::terminal::TerminalId::alloc(),
                123,
                file,
            )
            .unwrap();
        assert!(cache.take("pane_2", &target.target_id).is_none());
        let cached = cache.take("pane_1", &target.target_id).unwrap();
        assert!(cache.take("pane_1", &target.target_id).is_none());
        std::fs::rename(&path, dir.join("old")).unwrap();
        std::fs::write(&path, "replacement").unwrap();
        let (metadata, _) = crate::platform::sample_local_regular_file(&path).unwrap();
        assert!(!crate::platform::same_local_file(
            &cached.file.metadata,
            &metadata
        ));
        drop(cached);
        assert!(
            path.exists(),
            "dropping a real-file token must never delete it"
        );
        std::fs::write(dir.join("literal:12"), "exact").unwrap();
        assert_eq!(
            resolve_candidate("literal:12", &dir, None).unwrap().line,
            None
        );
        std::fs::write(dir.join("binary"), b"a\0b").unwrap();
        assert_eq!(
            resolve_candidate("binary", &dir, None).unwrap().kind,
            PaneFileKind::Binary
        );
        let file = resolve_candidate("literal:12", &dir, None).unwrap();
        let target = cache
            .insert(
                "pane_1".into(),
                crate::terminal::TerminalId::alloc(),
                123,
                file,
            )
            .unwrap();
        cache.entries.back_mut().unwrap().created = Instant::now() - Duration::from_secs(61);
        assert!(cache.take("pane_1", &target.target_id).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn file_resolution_quotes_unicode_and_canonical_safety() {
        let dir = std::env::temp_dir().join(format!(
            "herdr-file-test-{}",
            TargetCache::default().incarnation
        ));
        std::fs::create_dir(&dir).unwrap();
        for (name, quoted) in [
            ("a'b.rs", "\"a'b.rs\":12:3"),
            ("a\"b.rs", "'a\"b.rs':12:3"),
            ("a b.rs", "`a b.rs`:12:3"),
        ] {
            std::fs::write(dir.join(name), "text").unwrap();
            let candidate = candidates_at(quoted, 2).remove(0);
            let file = resolve_candidate(&candidate, &dir, None).unwrap();
            assert_eq!((file.line, file.column), (Some(12), Some(3)));
        }
        assert!(resolve_candidate("a b.rs:2147483648", &dir, None).is_none());
        assert!(resolve_candidate("a b.rs:1:2147483648", &dir, None).is_none());
        assert!(resolve_candidate("\"a b.rs\":2147483648:3", &dir, None).is_none());
        assert!(resolve_candidate("file://remote/tmp/a", &dir, None).is_none());
        let unsafe_path = dir.join("bad\nname");
        std::fs::write(&unsafe_path, "text").unwrap();
        std::os::unix::fs::symlink(&unsafe_path, dir.join("clean")).unwrap();
        assert!(resolve_candidate("clean", &dir, None).is_none());
        let unicode = format!("{}€tail", "a".repeat(8191));
        std::fs::write(dir.join("unicode"), &unicode).unwrap();
        assert_eq!(
            resolve_candidate("unicode", &dir, None).unwrap().kind,
            PaneFileKind::Text
        );
        std::fs::write(dir.join("incomplete"), &unicode.as_bytes()[..8192]).unwrap();
        assert_eq!(
            resolve_candidate("incomplete", &dir, None).unwrap().kind,
            PaneFileKind::Unknown
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn file_cache_incarnation_capacity_and_runtime_binding() {
        let mut first = TargetCache::default();
        let mut second = TargetCache::default();
        let dir = std::env::temp_dir().join(format!("herdr-file-test-{}", first.incarnation));
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("file"), "text").unwrap();
        let terminal = crate::terminal::TerminalId::alloc();
        let insert = |cache: &mut TargetCache| {
            cache
                .insert(
                    "pane_1".into(),
                    terminal.clone(),
                    123,
                    resolve_candidate("file", &dir, None).unwrap(),
                )
                .unwrap()
        };
        let first_target = insert(&mut first);
        let second_target = insert(&mut second);
        assert_ne!(first_target.target_id, second_target.target_id);
        assert!(second.take("pane_1", &first_target.target_id).is_none());
        let cached = second.take("pane_1", &second_target.target_id).unwrap();
        assert!(cached.matches_runtime(&terminal, 123));
        assert!(!cached.matches_runtime(&terminal, 124));
        assert!(!cached.matches_runtime(&crate::terminal::TerminalId::alloc(), 123));
        for _ in 0..64 {
            insert(&mut first);
        }
        assert_eq!(first.entries.len(), 64);
        assert!(first.take("pane_1", &first_target.target_id).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn file_path_candidates_and_schemes() {
        assert_eq!(candidates_at("see 'a b.rs' now", 7)[0], "'a b.rs'");
        assert_eq!(
            candidates_at("see `a b.rs`:12:3 now", 7)[0],
            "`a b.rs`:12:3"
        );
        assert_eq!(
            candidates_at("see \"a'b.rs\":12:3", 7)[0],
            "\"a'b.rs\":12:3"
        );
        assert_eq!(cursor_number("2147483648"), None);
        assert_eq!(cursor_number("2147483647"), Some(i32::MAX as u32));
        assert_eq!(
            decode_file_uri("file://localhost/tmp/a%20b"),
            Some("/tmp/a b".into())
        );
        assert!(decode_file_uri("file://remote/tmp/a").is_none());
        assert!(candidate_path("https://example/a", Path::new("/tmp"), None).is_none());
        assert!(candidate_path("file:///tmp/a%00b", Path::new("/tmp"), None).is_none());
        assert_eq!(classify(b"hello", 5), PaneFileKind::Text);
        assert_eq!(classify(b"a\0b", 3), PaneFileKind::Binary);
        assert_eq!(classify(&[255], 1), PaneFileKind::Unknown);
        assert_eq!(classify(&[0xe2, 0x82], 3), PaneFileKind::Text);
        assert_eq!(classify(&[0xe2, 0x82], 2), PaneFileKind::Unknown);
        assert_eq!(classify(&[255], 8193), PaneFileKind::Unknown);
        assert!(remote_program("/usr/bin/ssh"));
    }
}
