//! `co-review logs [--lines n]`: whether Co-Review answers (and how fast), then the recent debug log of every
//! Co-Review process, merged by time: what a person or an agent needs when a window hangs or a call fails. Never
//! starts Co-Review.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::app;
use crate::installation::{self, logs_directory, read_json};
use crate::process;

const TAIL_BYTES: u64 = 1024 * 1024;
const TAIL_LINES: usize = 4000;

pub fn run(lines: usize) {
    let home = installation::co_review_home();
    let mut output = vec![format!("co-review {}", installation::version())];
    output.extend(liveness(&home));
    output.extend(recent_entries(&logs_directory(&home), lines));
    println!("{}", output.join("\n"));
}

fn running(pid: Option<u64>) -> &'static str {
    let alive = pid
        .and_then(|pid| u32::try_from(pid).ok())
        .is_some_and(process::is_alive);
    if alive { "running" } else { "not running" }
}

fn display_pid(pid: Option<u64>) -> String {
    pid.map(|pid| pid.to_string())
        .unwrap_or_else(|| "undefined".to_string())
}

fn liveness(home: &Path) -> Vec<String> {
    let mut output = Vec::new();
    let app_record = read_json(&logs_directory(home).join("app.json"));
    if let Some(app_record) = app_record
        && let Some(pid) = app_record.get("pid").and_then(Value::as_u64)
    {
        let started = app_record
            .get("started")
            .and_then(Value::as_str)
            .unwrap_or("undefined");
        output.push(format!(
            "Desktop app: process {pid} {} (started {started})",
            running(Some(pid))
        ));
    }
    let server = read_json(&home.join("server.json"));
    let url = server
        .as_ref()
        .and_then(|server| server.get("url"))
        .and_then(Value::as_str);
    let Some(url) = url else {
        output.push(format!(
            "Backend: not running (no {})",
            home.join("server.json").display()
        ));
        return output;
    };
    let pid = server
        .as_ref()
        .and_then(|server| server.get("pid"))
        .and_then(Value::as_u64);
    let started = Instant::now();
    let answer = app::http_agent()
        .get(&format!("{url}/mcp"))
        .timeout(Duration::from_secs(3))
        .call();
    let milliseconds = started.elapsed().as_millis();
    output.push(match answer {
        Ok(_) | Err(ureq::Error::Status(..)) => {
            format!(
                "Backend: {url} (process {}) answers in {milliseconds} ms",
                display_pid(pid)
            )
        }
        Err(ureq::Error::Transport(transport))
            if transport.kind() == ureq::ErrorKind::Io && is_timeout(&transport) =>
        {
            format!(
                "Backend: {url} (process {}, {}) does NOT answer within 3 s: busy or stuck",
                display_pid(pid),
                running(pid)
            )
        }
        Err(ureq::Error::Transport(transport)) => format!(
            "Backend: {url} (process {}, {}) is not reachable: {transport}",
            display_pid(pid),
            running(pid)
        ),
    });
    output
}

fn is_timeout(transport: &ureq::Transport) -> bool {
    let text = transport.to_string();
    text.contains("timed out") || text.contains("timeout")
}

/// The end of a log file; started mid-file, without the partial first line (a log can be large).
fn tail(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    let length = size.min(TAIL_BYTES);
    file.seek(SeekFrom::Start(size - length)).ok()?;
    let mut bytes = Vec::with_capacity(length as usize);
    file.take(length).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    if length < size {
        return Some(
            text.split_once('\n')
                .map(|(_, rest)| rest.to_string())
                .unwrap_or_default(),
        );
    }
    Some(text)
}

/// `2026-01-01T00:00:00.000Z ` at the start of a line: an entry's stamp.
fn stamp(line: &str) -> Option<&str> {
    let bytes = line.as_bytes();
    let digits = |range: std::ops::Range<usize>| {
        range
            .into_iter()
            .all(|index| bytes.get(index).is_some_and(u8::is_ascii_digit))
    };
    if !(digits(0..4)
        && bytes.get(4) == Some(&b'-')
        && digits(5..7)
        && bytes.get(7) == Some(&b'-')
        && digits(8..10))
        || bytes.get(10) != Some(&b'T')
    {
        return None;
    }
    let mut end = 11;
    while bytes
        .get(end)
        .is_some_and(|byte| byte.is_ascii_digit() || *byte == b':' || *byte == b'.')
    {
        end += 1;
    }
    (end > 11 && bytes.get(end) == Some(&b'Z') && bytes.get(end + 1) == Some(&b' '))
        .then(|| &line[..end + 2])
}

struct Entry {
    at: String,
    name: &'static str,
    line: String,
}

fn is_problem(line: &str) -> bool {
    let lower = line.to_lowercase();
    line.contains(" WARN ")
        || line.contains(" ERROR ")
        || [
            "unresponsive",
            "not responding",
            "blocked for",
            "crash",
            "gone",
        ]
        .iter()
        .any(|word| lower.contains(word))
}

/// Entries of every log, by time; a line without a time stamp continues the entry before it.
fn recent_entries(directory: &Path, count: usize) -> Vec<String> {
    let mut entries: Vec<Entry> = Vec::new();
    for name in ["main", "backend", "cli"] {
        let Some(text) = tail(&directory.join(format!("{name}.log"))) else {
            continue;
        };
        let lines: Vec<&str> = text.split('\n').collect();
        for line in &lines[lines.len().saturating_sub(TAIL_LINES)..] {
            if let Some(at) = stamp(line) {
                entries.push(Entry {
                    at: at.to_string(),
                    name,
                    line: line.to_string(),
                });
            } else if !line.trim().is_empty()
                && let Some(last) = entries.last_mut()
                && last.name == name
            {
                last.line.push('\n');
                last.line.push_str(line);
            }
        }
    }
    entries.sort_by(|left, right| left.at.cmp(&right.at));
    let recent = &entries[entries.len().saturating_sub(count)..];
    let problems = recent
        .iter()
        .filter(|entry| is_problem(&entry.line))
        .count();
    let mut output = vec![
        format!(
            "Recent: {} entries, {problems} warnings or errors (logs: {})",
            recent.len(),
            directory.display()
        ),
        String::new(),
    ];
    output.extend(
        recent
            .iter()
            .map(|entry| format!("{:<7} {}", entry.name, entry.line)),
    );
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn logs_are_merged_by_time_with_continuation_lines() {
        let directory = tempfile::tempdir().expect("temporary directory");
        fs::write(
            directory.path().join("backend.log"),
            "2026-01-01T00:00:02.000Z 1 WARN  slow\n    detail\n2026-01-01T00:00:04.000Z 1 INFO  later\n",
        )
        .expect("write");
        fs::write(
            directory.path().join("cli.log"),
            "2026-01-01T00:00:01.000Z 2 INFO  mcp: first\n2026-01-01T00:00:03.000Z 2 INFO  mcp: lost Co-Review: gone\n",
        )
        .expect("write");
        let output = recent_entries(directory.path(), 3);
        assert_eq!(
            output[0],
            format!(
                "Recent: 3 entries, 2 warnings or errors (logs: {})",
                directory.path().display()
            )
        );
        assert_eq!(
            output[2..],
            [
                "backend 2026-01-01T00:00:02.000Z 1 WARN  slow\n    detail",
                "cli     2026-01-01T00:00:03.000Z 2 INFO  mcp: lost Co-Review: gone",
                "backend 2026-01-01T00:00:04.000Z 1 INFO  later",
            ]
        );
    }

    #[test]
    fn a_backend_not_running_names_the_server_record_of_this_home() {
        let home = tempfile::tempdir().expect("temporary directory");
        assert_eq!(
            liveness(home.path()),
            vec![format!(
                "Backend: not running (no {})",
                home.path().join("server.json").display()
            )]
        );
    }

    #[test]
    fn stamps_are_recognized() {
        assert_eq!(
            stamp("2026-01-01T00:00:02.000Z 1 INFO x"),
            Some("2026-01-01T00:00:02.000Z ")
        );
        assert_eq!(stamp("    2026-01-01T00:00:02.000Z"), None);
        assert_eq!(stamp("2026-01-01T00:00:02.000Z"), None);
    }
}
