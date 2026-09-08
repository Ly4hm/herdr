//! Recognize local path text under a terminal display column, without filesystem I/O.

use std::sync::LazyLock;

use regex::Regex;
use unicode_width::UnicodeWidthChar;

/// A recognized path and its visible byte range in the input logical line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathMatch {
    pub path: String,
    pub byte_range: std::ops::Range<usize>,
}

/// Find a local path under a zero-based terminal display column.
///
/// Quoted paths may contain spaces. Markdown links also match clicks on their
/// label. Input must be one logical line, without ANSI escape sequences.
pub fn path_at_column(line: &str, col: u16) -> Option<String> {
    path_match_at_column(line, col).map(|matched| matched.path)
}

/// Find a path and the exact byte span to underline at a display column.
/// Source location suffixes remain in the span; prose punctuation and quotes do
/// not. Markdown matches span the entire link, including its label.
pub fn path_match_at_column(line: &str, col: u16) -> Option<PathMatch> {
    let clicked = byte_at_column(line, col)?;

    // Link destinations can contain spaces inside angle brackets. Nested link
    // labels and parentheses in destinations are intentionally left to quotes.
    static MARKDOWN: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"\[[^\]\r\n]*\]\((?:<([^>\r\n]+)>|([^()\r\n]+))\)").ok());
    if let Some(regex) = MARKDOWN.as_ref() {
        for captures in regex.captures_iter(line) {
            let Some(whole) = captures.get(0) else {
                continue;
            };
            if whole.range().contains(&clicked) {
                let destination = captures.get(1).or_else(|| captures.get(2))?;
                return Some(PathMatch {
                    path: clean_path(destination.as_str(), false)?,
                    byte_range: whole.range(),
                });
            }
        }
    }

    let mut chars = line.char_indices().peekable();
    while let Some((start, ch)) = chars.next() {
        if ch.is_whitespace() || is_boundary(ch) {
            continue;
        }
        if matches!(ch, '\'' | '"' | '`') {
            let content_start = start + ch.len_utf8();
            let mut end = line.len();
            for (index, next) in chars.by_ref() {
                if next == ch {
                    end = index;
                    break;
                }
            }
            if (content_start..end).contains(&clicked) {
                return match_token(&line[content_start..end], content_start, clicked, false);
            }
            continue;
        }
        let mut end = start + ch.len_utf8();
        while let Some(&(index, next)) = chars.peek() {
            if next.is_whitespace() || is_boundary(next) {
                break;
            }
            end = index + next.len_utf8();
            chars.next();
        }
        if (start..end).contains(&clicked) {
            return match_token(&line[start..end], start, clicked, true);
        }
    }
    None
}

/// Extract an absolute local path from a `file://` hyperlink.
///
/// Only empty and localhost authorities are accepted. URI query and fragment
/// text is excluded before percent decoding, so encoded filename punctuation is
/// preserved. Malformed escapes, invalid UTF-8, and NUL bytes are rejected.
pub fn path_from_file_uri(uri: &str) -> Option<String> {
    let (scheme, remainder) = uri.split_once(':')?;
    if !scheme.eq_ignore_ascii_case("file") || uri.contains('\0') {
        return None;
    }
    let (authority, raw_path) = remainder.strip_prefix("//")?.split_once('/')?;
    if !authority.is_empty() && !authority.eq_ignore_ascii_case("localhost") {
        return None;
    }
    let raw_path = raw_path.split(['?', '#']).next()?;
    let mut bytes = Vec::with_capacity(raw_path.len() + 1);
    bytes.push(b'/');
    let mut source = raw_path.bytes();
    while let Some(byte) = source.next() {
        let decoded = if byte == b'%' {
            let high = char::from(source.next()?).to_digit(16)? as u8;
            let low = char::from(source.next()?).to_digit(16)? as u8;
            (high << 4) | low
        } else {
            byte
        };
        if decoded == 0 {
            return None;
        }
        bytes.push(decoded);
    }
    let path = String::from_utf8(bytes).ok()?;
    // A decoded leading double slash could turn into a remote/UNC path later.
    if path.starts_with("//") || path.starts_with("/\\") {
        return None;
    }
    // Windows file URIs conventionally spell drive paths as /C:/directory.
    if path.as_bytes().get(1).is_some_and(u8::is_ascii_alphabetic)
        && path.as_bytes().get(2) == Some(&b':')
        && path.as_bytes().get(3) == Some(&b'/')
    {
        return Some(path[1..].to_owned());
    }
    Some(path)
}

fn byte_at_column(line: &str, col: u16) -> Option<usize> {
    let mut column = 0usize;
    for (byte, ch) in line.char_indices() {
        let width = ch.width().unwrap_or(0);
        if column <= usize::from(col) && usize::from(col) < column + width {
            return Some(byte);
        }
        column += width;
    }
    None
}

fn is_boundary(ch: char) -> bool {
    matches!(
        ch,
        '<' | '>'
            | '|'
            | '='
            | '['
            | ']'
            | '{'
            | '}'
            | '('
            | ')'
            | '：'
            | '，'
            | '；'
            | '。'
            | '！'
            | '？'
            | '、'
            | '（'
            | '）'
            | '【'
            | '】'
    )
}

fn path_token(raw: &str, trim_punctuation: bool) -> &str {
    let raw = raw.trim().trim_matches(['`', '\'', '"']);
    if trim_punctuation {
        raw.trim_end_matches(['.', ',', ';', '!', '?', '，', '。', '；', '！', '？', ':'])
    } else {
        raw
    }
}

fn match_token(
    raw: &str,
    start: usize,
    clicked: usize,
    trim_punctuation: bool,
) -> Option<PathMatch> {
    let path = clean_path(raw, trim_punctuation)?;
    let token = path_token(raw, trim_punctuation);
    let byte_start = start + raw.find(token)?;
    let byte_range = byte_start..byte_start + token.len();
    if !byte_range.contains(&clicked) {
        return None;
    }
    Some(PathMatch { path, byte_range })
}

fn clean_path(raw: &str, trim_punctuation: bool) -> Option<String> {
    let raw = path_token(raw, trim_punctuation);
    // Strip source locations, but preserve the colon in a Windows drive prefix.
    let mut path = raw;
    for _ in 0..2 {
        let Some((prefix, suffix)) = path.rsplit_once(':') else {
            break;
        };
        if !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()) {
            path = prefix;
        } else {
            break;
        }
    }
    if path.is_empty() || path.contains("://") || path.starts_with("//") {
        return None;
    }
    let windows_drive = path.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && path.as_bytes().get(1) == Some(&b':')
        && path
            .as_bytes()
            .get(2)
            .is_some_and(|byte| matches!(byte, b'/' | b'\\'));
    let unix_path = path.contains('/') && !path.contains(':');
    if windows_drive || unix_path {
        Some(path.to_owned())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    fn at(line: &str, needle: &str) -> Option<String> {
        let index = line.find(needle).expect("test needle exists");
        path_at_column(line, line[..index].width() as u16)
    }

    #[test]
    fn decodes_local_file_uri_paths_without_losing_filename_punctuation() {
        for (uri, expected) in [
            ("file:///home/me/my%20file.rs#L12", "/home/me/my file.rs"),
            ("file://localhost/tmp/%E6%96%87%E4%BB%B6", "/tmp/文件"),
            ("FILE://LOCALHOST/tmp/a%23b%3Fc%25d?ignored", "/tmp/a#b?c%d"),
            ("file:///tmp/a+b", "/tmp/a+b"),
            ("file:///", "/"),
            ("file:///C:/My%20Files/a.txt", "C:/My Files/a.txt"),
        ] {
            assert_eq!(path_from_file_uri(uri).as_deref(), Some(expected), "{uri}");
        }
    }

    #[test]
    fn rejects_remote_and_malformed_file_uris() {
        for uri in [
            "https:///tmp/a",
            "file://server/tmp/a",
            "file://user@localhost/tmp/a",
            "file://localhost:22/tmp/a",
            "file:///tmp/%",
            "file:///tmp/%0",
            "file:///tmp/%GG",
            "file:///tmp/%ff",
            "file:///tmp/%00",
            "file:///tmp/a\0",
            "file:relative",
            "file:////server/share",
            "file:///%2Fserver/share",
            "file:///%5Cserver/share",
        ] {
            assert_eq!(path_from_file_uri(uri), None, "{uri:?}");
        }
    }

    #[test]
    fn recognizes_local_forms_and_source_locations() {
        for (text, expected) in [
            ("./src/main.rs:12:3", "./src/main.rs"),
            ("../src/main.rs:12", "../src/main.rs"),
            ("/home/user/code/", "/home/user/code/"),
            ("~/project/file", "~/project/file"),
            ("src/main.rs,", "src/main.rs"),
            (r"C:\Users\me\file.rs:12:3", r"C:\Users\me\file.rs"),
            ("C:/Users/me/file.rs", "C:/Users/me/file.rs"),
        ] {
            assert_eq!(path_at_column(text, 2).as_deref(), Some(expected), "{text}");
        }
    }

    #[test]
    fn path_spans_exclude_prose_quotes_and_trailing_punctuation() {
        for (line, needle, expected_span, expected_path) in [
            (
                "• 文件：/home/herdr-usage/README.md,",
                "README",
                "/home/herdr-usage/README.md",
                "/home/herdr-usage/README.md",
            ),
            (
                "源文件：./中文目录/说明.md:12:3.",
                "说明",
                "./中文目录/说明.md:12:3",
                "./中文目录/说明.md",
            ),
            (
                "查看 `./my project/文件.rs:42`",
                "project",
                "./my project/文件.rs:42",
                "./my project/文件.rs",
            ),
            (
                "[源文件](./src/main.rs:20)",
                "源文件",
                "[源文件](./src/main.rs:20)",
                "./src/main.rs",
            ),
            (
                "[源文件](./src/main.rs:20)",
                "main",
                "[源文件](./src/main.rs:20)",
                "./src/main.rs",
            ),
        ] {
            let column = line[..line.find(needle).expect("needle exists")].width() as u16;
            let matched = path_match_at_column(line, column).expect("path matched");
            assert_eq!(&line[matched.byte_range], expected_span, "{line}");
            assert_eq!(matched.path, expected_path, "{line}");
        }
        let line = "文件：./目录/文件.md,";
        for needle in ["文件：", "：", ","] {
            let column = line[..line.find(needle).expect("needle exists")].width() as u16;
            assert_eq!(path_match_at_column(line, column), None);
        }
    }

    #[test]
    fn matches_the_clicked_token_only() {
        let line = "Read ./one.txt and ../two.txt.";
        assert_eq!(at(line, "one").as_deref(), Some("./one.txt"));
        assert_eq!(at(line, "two").as_deref(), Some("../two.txt"));
        assert_eq!(at(line, "and"), None);
        assert_eq!(path_at_column(line, 4), None);
        assert_eq!(path_at_column(line, 200), None);
    }

    #[test]
    fn separates_chinese_prose_labels_from_clicked_paths() {
        for (line, label, path) in [
            (
                "• 文件：/home/herdr-usage/README.md",
                "文件",
                "/home/herdr-usage/README.md",
            ),
            ("  目录：/home/herdr-usage", "目录", "/home/herdr-usage"),
            ("文件：./中文目录/说明.md", "文件", "./中文目录/说明.md"),
            (
                r"文件：C:\Users\中文目录\说明.md:12:3",
                "文件",
                r"C:\Users\中文目录\说明.md",
            ),
        ] {
            assert_eq!(at(line, label), None, "{line}");
            assert_eq!(at(line, "："), None, "{line}");
            assert_eq!(at(line, path).as_deref(), Some(path), "{line}");
        }
        for punctuation in ['，', '；', '。', '！', '？', '、', '（', '【'] {
            let line = format!("请查看{punctuation}./中文目录/说明.md");
            assert_eq!(at(&line, "查看"), None);
            assert_eq!(at(&line, "说明").as_deref(), Some("./中文目录/说明.md"));
        }
    }

    #[test]
    fn chinese_prose_delimiters_inside_quotes_remain_filename_characters() {
        let path = "./中文目录/文件：甲，乙；丙。md";
        for quote in ['`', '\'', '"'] {
            let line = format!("文件：{quote}{path}{quote}");
            assert_eq!(at(&line, "甲").as_deref(), Some(path));
        }
        assert_eq!(
            at(&format!("[文件]({path})"), "文件").as_deref(),
            Some(path)
        );
    }

    #[test]
    fn uses_display_columns_for_chinese_and_combining_marks() {
        let line = "查看 e\u{301} ./目录/文件.rs";
        assert_eq!(at(line, "文件").as_deref(), Some("./目录/文件.rs"));
        let column = line[..line.find('文').expect("character exists")].width() as u16;
        assert_eq!(
            path_at_column(line, column + 1).as_deref(),
            Some("./目录/文件.rs")
        );
    }

    #[test]
    fn quoted_paths_preserve_spaces() {
        for quote in ['`', '\'', '"'] {
            let line = format!("open {quote}./my project/文件.rs:42{quote} now");
            assert_eq!(
                at(&line, "project").as_deref(),
                Some("./my project/文件.rs")
            );
        }
        assert_eq!(
            at(r#"open "C:\My Files\a.txt""#, "Files").as_deref(),
            Some(r"C:\My Files\a.txt")
        );
    }

    #[test]
    fn preserves_quoted_and_markdown_filename_punctuation() {
        assert_eq!(
            at("`./directory/file.`", "file").as_deref(),
            Some("./directory/file.")
        );
        assert_eq!(
            at("[file](./directory/file!)", "file").as_deref(),
            Some("./directory/file!")
        );
        assert_eq!(
            at("Here's ./directory/file.", "directory").as_deref(),
            Some("./directory/file")
        );
    }

    #[test]
    fn markdown_links_match_label_and_destination() {
        let line = "See [源文件](./src/main.rs:20) please";
        assert_eq!(at(line, "源").as_deref(), Some("./src/main.rs"));
        assert_eq!(at(line, "main").as_deref(), Some("./src/main.rs"));
        assert_eq!(
            at("[open](<./my project/file>)", "open").as_deref(),
            Some("./my project/file")
        );
    }

    #[test]
    fn rejects_urls_and_ordinary_words() {
        for line in [
            "https://example.org/file",
            "http://localhost/a",
            "file:///tmp/a",
            "//example.org/path",
            "word",
            "main.rs",
            "12:34",
        ] {
            assert_eq!(path_at_column(line, 2), None, "{line}");
        }
        assert_eq!(at("[web](https://example.org/a)", "web"), None);
    }
}
