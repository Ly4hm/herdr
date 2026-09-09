//! Read the working directory declared by one exact Codex session header.
use std::fs;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::agent_resume::AgentSessionRefKind;
use crate::api::schema::AgentSessionInfo;

const MAX_DIRECTORY_ENTRIES: usize = 4096;
const MAX_HEADER_BYTES: u64 = 64 * 1024;

/// Resolve only an identified Codex session, never the newest matching project.
/// Missing sessions return `None`; ambiguous or malformed records are errors.
pub(crate) fn agent_session_cwd(session: &AgentSessionInfo) -> io::Result<Option<PathBuf>> {
    if session.agent != "codex" || session.kind != AgentSessionRefKind::Id {
        return Ok(None);
    }
    session_cwd_in(&super::env::codex_dir()?, &session.value)
}

fn invalid_data(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn session_date(id: &str) -> io::Result<time::Date> {
    let bytes = id.as_bytes();
    if bytes.len() != 36
        || [8, 13, 18, 23].iter().any(|&index| bytes[index] != b'-')
        || bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| ![8, 13, 18, 23].contains(&index) && !byte.is_ascii_hexdigit())
        || bytes[14] != b'7'
        || !matches!(bytes[19], b'8' | b'9' | b'a' | b'A' | b'b' | b'B')
    {
        return Err(invalid_data("Codex session ID must be a UUIDv7"));
    }
    let mut timestamp_ms = 0i64;
    for &byte in bytes.iter().take(13).filter(|&&byte| byte != b'-') {
        let digit = char::from(byte)
            .to_digit(16)
            .ok_or_else(|| invalid_data("invalid session timestamp"))?;
        timestamp_ms = (timestamp_ms << 4) | i64::from(digit);
    }
    time::OffsetDateTime::from_unix_timestamp(timestamp_ms / 1000)
        .map(|timestamp| timestamp.date())
        .map_err(|_| invalid_data("Codex session timestamp is out of range"))
}

fn session_cwd_in(codex_home: &Path, id: &str) -> io::Result<Option<PathBuf>> {
    let date = session_date(id)?;
    let id = id.to_ascii_lowercase();
    let mut directories = Vec::with_capacity(4);
    for offset in [-1, 0, 1] {
        let day = date
            .checked_add(time::Duration::days(offset))
            .ok_or_else(|| invalid_data("Codex session date is out of range"))?;
        directories.push(codex_home.join("sessions").join(format!(
            "{:04}/{:02}/{:02}",
            day.year(),
            u8::from(day.month()),
            day.day()
        )));
    }
    directories.push(codex_home.join("archived_sessions"));
    let suffix = format!("-{id}.jsonl");
    let mut candidate = None;
    let mut entry_count = 0;
    for directory in directories {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for entry in entries {
            entry_count += 1;
            if entry_count > MAX_DIRECTORY_ENTRIES {
                return Err(invalid_data("Codex session directory scan limit exceeded"));
            }
            let entry = entry?;
            let filename = entry.file_name();
            let Some(filename) = filename.to_str() else {
                continue;
            };
            if !filename.starts_with("rollout-") || !filename.ends_with(&suffix) {
                continue;
            }
            if !entry.file_type()?.is_file() {
                return Err(invalid_data("Codex session header must be a regular file"));
            }
            if candidate.replace(entry.path()).is_some() {
                return Err(invalid_data("multiple files match the Codex session ID"));
            }
        }
    }
    candidate
        .map(|path| read_session_header(&path, &id))
        .transpose()
}

#[derive(Deserialize)]
struct SessionHeader {
    #[serde(rename = "type")]
    kind: String,
    payload: SessionMetadata,
}

#[derive(Deserialize)]
struct SessionMetadata {
    id: String,
    cwd: String,
}

fn read_session_header(path: &Path, expected_id: &str) -> io::Result<PathBuf> {
    let file = fs::File::open(path)?;
    // The limit applies before buffering. Only the first JSONL record is parsed.
    let mut reader = BufReader::new(file.take(MAX_HEADER_BYTES));
    let mut line = Vec::new();
    reader.read_until(b'\n', &mut line)?;
    if line.len() as u64 == MAX_HEADER_BYTES && line.last() != Some(&b'\n') {
        return Err(invalid_data("Codex session header exceeds 64 KiB"));
    }
    let header: SessionHeader = serde_json::from_slice(&line)
        .map_err(|_| invalid_data("malformed Codex session header"))?;
    if header.kind != "session_meta" || !header.payload.id.eq_ignore_ascii_case(expected_id) {
        return Err(invalid_data(
            "Codex session header ID or record type does not match",
        ));
    }
    let cwd = PathBuf::from(&header.payload.cwd);
    if !cwd.is_absolute() || header.payload.cwd.contains('\0') {
        return Err(invalid_data(
            "Codex session working directory is not an absolute path",
        ));
    }
    Ok(cwd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    const ID: &str = "01a08517-0000-7000-8000-000000000001";
    const OTHER_ID: &str = "01a08528-0000-7000-8000-000000000002";
    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "herdr-session-cwd-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn write(&self, directory: &str, filename_id: &str, contents: &str) -> PathBuf {
            let directory = self.0.join(directory);
            fs::create_dir_all(&directory).unwrap();
            let path = directory.join(format!("rollout-2026-09-09T01-00-00-{filename_id}.jsonl"));
            fs::write(&path, contents).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn header(id: &str, cwd: &Path) -> String {
        serde_json::json!({"type":"session_meta","payload":{"id":id,"cwd":cwd}}).to_string() + "\n"
    }

    #[test]
    fn exact_session_id_selects_its_header_and_ignores_other_projects_and_body() {
        let fixture = Fixture::new();
        let cwd = fixture.0.join("中文 project");
        fixture.write("sessions/2026/09/09", OTHER_ID, "corrupt other session\n");
        fixture.write(
            "sessions/2026/09/09",
            ID,
            &(header(ID, &cwd) + "not JSON chat body; never parsed\n"),
        );
        assert_eq!(session_cwd_in(&fixture.0, ID).unwrap(), Some(cwd));
    }

    #[test]
    fn rejects_mismatched_ids_forged_body_corruption_and_relative_directories() {
        let fixture = Fixture::new();
        let absolute = fixture.0.join("project");
        for text in [
            header(OTHER_ID, &absolute),
            "{\"type\":\"event_msg\",\"payload\":{\"message\":\"session_meta\"}}\n".to_owned()
                + &header(ID, &absolute),
            "broken JSON\n".into(),
            header(ID, Path::new("relative/project")),
            header(ID, Path::new("/tmp/a\0b")),
        ] {
            fixture.write("sessions/2026/09/09", ID, &text);
            assert!(session_cwd_in(&fixture.0, ID).is_err());
        }
    }

    #[test]
    fn date_neighbors_and_archived_sessions_are_supported_but_duplicates_rejected() {
        for directory in [
            "sessions/2026/09/08",
            "sessions/2026/09/09",
            "sessions/2026/09/10",
            "archived_sessions",
        ] {
            let fixture = Fixture::new();
            let cwd = fixture.0.join("project");
            fixture.write(directory, ID, &header(ID, &cwd));
            assert_eq!(session_cwd_in(&fixture.0, ID).unwrap(), Some(cwd.clone()));
            let other = if directory == "archived_sessions" {
                "sessions/2026/09/09"
            } else {
                "archived_sessions"
            };
            fixture.write(other, ID, &header(ID, &cwd));
            assert!(session_cwd_in(&fixture.0, ID).is_err());
        }
    }

    #[test]
    fn invalid_ids_missing_sessions_and_oversized_headers_are_bounded() {
        let fixture = Fixture::new();
        assert_eq!(session_cwd_in(&fixture.0, ID).unwrap(), None);
        for id in [
            "latest",
            "../session",
            "01a08517-0000-4000-8000-000000000001",
        ] {
            assert!(session_cwd_in(&fixture.0, id).is_err());
        }
        fixture.write(
            "sessions/2026/09/09",
            ID,
            &" ".repeat(MAX_HEADER_BYTES as usize + 1),
        );
        assert!(session_cwd_in(&fixture.0, ID).is_err());
        assert_eq!(session_date(ID).unwrap().to_string(), "2026-09-09");
    }

    #[test]
    fn ignores_other_agents_and_non_id_session_references() {
        let mut session = AgentSessionInfo {
            source: "test".into(),
            agent: "claude".into(),
            kind: AgentSessionRefKind::Id,
            value: ID.into(),
        };
        assert_eq!(agent_session_cwd(&session).unwrap(), None);
        session.agent = "codex".into();
        session.kind = AgentSessionRefKind::Path;
        assert_eq!(agent_session_cwd(&session).unwrap(), None);
    }
}
