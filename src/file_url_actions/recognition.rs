//! Bounded token recognition at a clicked UTF-8 byte offset. No I/O or hover work.

pub(crate) fn valid_web_url(value: &str) -> bool {
    if value.len() > 8192
        || value.chars().any(|c| c.is_control() || c.is_whitespace())
        || !(value
            .get(..7)
            .is_some_and(|s| s.eq_ignore_ascii_case("http://"))
            || value
                .get(..8)
                .is_some_and(|s| s.eq_ignore_ascii_case("https://")))
    {
        return false;
    }
    // Parse for validation only. Keep the original spelling, query, and fragment
    // in the target instead of replacing it with a normalized URL.
    url::Url::parse(value).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some_and(|host| !host.is_empty())
    })
}

fn line_at(text: &str, byte: usize) -> Option<(&str, usize)> {
    if byte >= text.len() || !text.is_char_boundary(byte) {
        return None;
    }
    let start = text[..byte].rfind('\n').map_or(0, |n| n + 1);
    let end = text[byte..].find('\n').map_or(text.len(), |n| byte + n);
    let line = &text[start..end];
    (line.len() <= 16384).then_some((line, byte - start))
}

fn trim_url_end(value: &str) -> usize {
    let mut end = value.len();
    while let Some(ch) = value[..end].chars().next_back() {
        let trim = match ch {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' | '`' => true,
            ')' | ']' | '}' => {
                let open = match ch {
                    ')' => '(',
                    ']' => '[',
                    _ => '{',
                };
                let balance = value[..end]
                    .chars()
                    .fold(0i32, |n, c| n + i32::from(c == open) - i32::from(c == ch));
                balance < 0
            }
            _ => false,
        };
        if !trim {
            break;
        }
        end -= ch.len_utf8();
    }
    end
}

pub(crate) fn web_url_at(text: &str, byte: usize) -> Option<String> {
    let (line, clicked) = line_at(text, byte)?;
    for (start, _) in line.char_indices() {
        let rest = &line[start..];
        if !(rest
            .get(..7)
            .is_some_and(|s| s.eq_ignore_ascii_case("http://"))
            || rest
                .get(..8)
                .is_some_and(|s| s.eq_ignore_ascii_case("https://")))
        {
            continue;
        }
        let quote = line[..start]
            .chars()
            .next_back()
            .filter(|c| matches!(c, '\'' | '"' | '`'));
        let mut end = rest
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>') || quote == Some(c))
            .unwrap_or(rest.len());
        // A closing quote defines the exact span; otherwise trim common prose
        // punctuation, preserving balanced parentheses within the URL.
        if quote.is_none() || rest[end..].chars().next() != quote {
            end = trim_url_end(&rest[..end]);
        }
        if clicked >= start && clicked < start + end && valid_web_url(&rest[..end]) {
            return Some(rest[..end].to_owned());
        }
    }
    None
}

/// Exact raw filenames remain first so punctuation in a real filename wins.
/// A trimmed fallback is eligible only when the pointer is inside that span.
pub(crate) fn file_candidates_at(text: &str, byte: usize) -> Vec<String> {
    let Some((row, clicked)) = line_at(text, byte) else {
        return Vec::new();
    };
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
                if clicked == begin || clicked == i {
                    return Vec::new();
                }
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
    if row[clicked..]
        .chars()
        .next()
        .is_none_or(char::is_whitespace)
    {
        return Vec::new();
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
    let mut candidates = vec![raw.to_owned()];
    let trimmed_start = raw.trim_start_matches(['\'', '"', '`', '(', '[', '{']);
    let leading = raw.len() - trimmed_start.len();
    let trimmed = trimmed_start.trim_end_matches(['\'', '"', '`', ')', ']', '}', ',', ';', '.']);
    if !trimmed.is_empty()
        && trimmed != raw
        && clicked >= left + leading
        && clicked < left + leading + trimmed.len()
    {
        candidates.push(trimmed.to_owned());
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_validation_is_syntax_only_and_does_not_normalize_original_text() {
        for value in [
            "https://example.com/path?q=one%20two&b=x;y#section",
            "HTTP://localhost:1/",
            "https://[::1]:8/a",
            "https://例え.jp/文",
        ] {
            assert!(valid_web_url(value), "{value}");
            for (byte, _) in value.char_indices() {
                assert_eq!(
                    web_url_at(value, byte).as_deref(),
                    Some(value),
                    "{value} at {byte}"
                );
            }
        }
        for value in [
            "https://",
            "https:///",
            "https://:bad/",
            "https://[bad]/",
            "javascript:alert(1)",
            "file:///tmp/a",
            "https://example.com/a\nb",
            "https://exa mple.com/",
            "https:example.com",
        ] {
            assert!(!valid_web_url(value), "{value}");
        }
    }

    #[test]
    fn punctuation_outside_url_is_not_a_target_but_balanced_closers_are() {
        let target = "https://example.com/wiki/(word)?q=x;y#z";
        let text = format!("See ({target}), then stop.");
        let start = text.find(target).unwrap();
        for (byte, _) in target.char_indices() {
            assert_eq!(web_url_at(&text, start + byte).as_deref(), Some(target));
        }
        for byte in [start - 1, start + target.len(), start + target.len() + 1] {
            assert!(web_url_at(&text, byte).is_none());
        }
        let quoted = "\"https://example.com/?q=what?\"";
        assert_eq!(
            web_url_at(quoted, 3).as_deref(),
            Some("https://example.com/?q=what?")
        );
    }

    #[test]
    fn quoted_paths_accept_spaces_and_positions_but_not_delimiters() {
        let text = "before 'space 文 file.txt':42:7 after";
        let start = text.find('\'').unwrap();
        let end = text.find(" after").unwrap();
        let close = text[start + 1..].find('\'').unwrap() + start + 1;
        for (byte, _) in text
            .char_indices()
            .filter(|(b, _)| *b > start && *b < end && *b != close)
        {
            assert_eq!(
                file_candidates_at(text, byte),
                vec!["'space 文 file.txt':42:7"],
                "byte {byte}"
            );
        }
        assert!(file_candidates_at(text, start).is_empty());
        assert!(file_candidates_at(text, close).is_empty());
    }

    #[test]
    fn filename_punctuation_fallback_requires_pointer_inside_span() {
        let text = "(a.rs:3:1),";
        for byte in 1..9 {
            assert!(file_candidates_at(text, byte).contains(&"a.rs:3:1".to_owned()));
        }
        assert_eq!(file_candidates_at(text, 0), vec![text]);
        assert_eq!(file_candidates_at(text, 9), vec![text]);
        assert_eq!(file_candidates_at(text, 10), vec![text]);
        assert_eq!(file_candidates_at("a", 0), vec!["a"]);
        assert!(file_candidates_at("a b", 1).is_empty());
    }

    #[test]
    fn hard_lines_and_bounds_are_preserved() {
        assert_eq!(file_candidates_at("abc\ndef", 5), vec!["def"]);
        assert!(web_url_at("https://example.\ncom", 20).is_none());
        assert!(file_candidates_at("文", 1).is_empty());
        assert!(file_candidates_at(&"a".repeat(16385), 5).is_empty());
        assert!(web_url_at(&format!("https://x/{}", "a".repeat(8192)), 5).is_none());
    }
}
