//! Co-Review's debug logs, for when something hangs or fails: `$CO_REVIEW_HOME/logs/<name>.log` (default
//! `~/.co-review/logs`), one file per process kind (`main`: the desktop app's windows; `backend`; `cli`: the
//! `co-review` command and each `co-review mcp`), every line stamped with the time and process id. `co-review logs`
//! prints the end of them merged by time, with whether Co-Review answers.
//!
//! A log never grows without bound, whatever floods it: a file is moved to `<name>.1.log` once it passes 5 MB, an
//! entry repeated right after itself (the same but for its numbers: an id, a count) is counted instead of written
//! again, at most 50 entries a second are written, and an entry is cut at 4 KB.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::installation;

const MAX_BYTES: u64 = 5 * 1024 * 1024;
const MAX_ENTRY: usize = 4 * 1024;
const MAX_PER_SECOND: u32 = 50;
const REPEAT_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

/// `2026-01-01T00:00:00.000Z`, as JavaScript's `Date.toISOString`.
pub fn timestamp() -> String {
    let now: chrono::DateTime<chrono::Utc> = SystemTime::now().into();
    now.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// One log line (or several, the rest indented), as every Co-Review process writes them.
pub fn format_line(level: Level, body: &str) -> String {
    format!(
        "{} {} {:<5} {}\n",
        timestamp(),
        std::process::id(),
        level.label(),
        body.split('\n').collect::<Vec<_>>().join("\n    ")
    )
}

pub struct DebugLog {
    state: Mutex<State>,
}

struct State {
    name: String,
    directory: PathBuf,
    file: Option<File>,
    written: u64,
    last: Option<LastEntry>,
    /// The body of the last entry written (not a count), to tell whether repeats differed from it.
    last_body: String,
    second: u64,
    in_second: u32,
    dropped: u32,
}

/// The last entry written, with its numbers taken out, how many times it came again since, and the latest of those.
struct LastEntry {
    level: Level,
    key: String,
    repeats: u32,
    since: Instant,
    latest: String,
}

impl DebugLog {
    /// The log `<logs>/<name>.log`; logging does nothing when the file cannot be opened (no logs, rather than no app).
    pub fn new(name: &str, logs_directory: &Path) -> Self {
        let mut state = State {
            name: name.to_string(),
            directory: logs_directory.to_path_buf(),
            file: None,
            written: 0,
            last: None,
            last_body: String::new(),
            second: 0,
            in_second: 0,
            dropped: 0,
        };
        if fs::create_dir_all(logs_directory).is_ok() {
            state.written = fs::metadata(state.path())
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            state.open();
        }
        DebugLog {
            state: Mutex::new(state),
        }
    }

    /// The log of this kind of process in `$CO_REVIEW_HOME/logs`.
    pub fn named(name: &str) -> Self {
        Self::new(
            name,
            &installation::logs_directory(&installation::co_review_home()),
        )
    }

    pub fn info(&self, text: &str) {
        self.write(Level::Info, text);
    }

    pub fn warn(&self, text: &str) {
        self.write(Level::Warn, text);
    }

    pub fn error(&self, text: &str) {
        self.write(Level::Error, text);
    }

    /// One entry; a multi-line text keeps the stamp on its first line and indents the rest.
    pub fn write(&self, level: Level, text: &str) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.file.is_none() {
            return;
        }
        let mut body = text.trim_end().to_string();
        let characters = body.chars().count();
        if characters > MAX_ENTRY {
            body = format!(
                "{} … ({} more characters)",
                body.chars().take(MAX_ENTRY).collect::<String>(),
                characters - MAX_ENTRY
            );
        }
        // The same entry again (a loop), but for its numbers: counted, and written once it stops or every 10 s.
        let key = without_numbers(&body);
        if let Some(last) = state.last.as_mut()
            && last.level == level
            && last.key == key
        {
            last.repeats += 1;
            last.latest = body.clone();
            if last.since.elapsed() < REPEAT_INTERVAL {
                return;
            }
        }
        // Different entries faster than anyone reads them: at most 50 a second, and how many were dropped.
        let second = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0);
        if second != state.second {
            state.flush_dropped();
            state.second = second;
            state.in_second = 0;
        }
        state.in_second += 1;
        if state.in_second > MAX_PER_SECOND {
            state.dropped += 1;
            return;
        }
        state.flush_repeats();
        state.last = Some(LastEntry {
            level,
            key,
            repeats: 0,
            since: Instant::now(),
            latest: body.clone(),
        });
        state.append(level, &body);
    }
}

impl Drop for DebugLog {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            state.flush_dropped();
            state.flush_repeats();
        }
    }
}

impl State {
    fn path(&self) -> PathBuf {
        self.directory.join(format!("{}.log", self.name))
    }

    fn open(&mut self) {
        if self.written > MAX_BYTES {
            let rotated = self.directory.join(format!("{}.1.log", self.name));
            if fs::rename(self.path(), rotated).is_err() {
                self.file = None;
                return;
            }
            self.written = 0;
        }
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path())
            .ok();
    }

    fn flush_dropped(&mut self) {
        if self.dropped > 0 {
            self.flush_repeats();
            self.last = None;
            let text = format!(
                "({} more entries dropped in a second: too many)",
                self.dropped
            );
            self.append(Level::Warn, &text);
            self.dropped = 0;
        }
    }

    /// The count of repeats, with the latest of them when they differed in their numbers (how long, which id).
    fn flush_repeats(&mut self) {
        let Some(last) = self.last.as_mut() else {
            return;
        };
        if last.repeats == 0 {
            return;
        }
        let (level, repeats, latest) = (last.level, last.repeats, last.latest.clone());
        last.repeats = 0;
        last.since = Instant::now();
        let plural = if repeats == 1 { "" } else { "s" };
        let text = if latest != self.last_body {
            format!("(the entry above came {repeats} more time{plural}, the last: {latest})")
        } else {
            format!("(the entry above came {repeats} more time{plural})")
        };
        self.append(level, &text);
    }

    fn append(&mut self, level: Level, body: &str) {
        if !body.starts_with("(the entry above came") {
            self.last_body = body.to_string();
        }
        let line = format_line(level, body);
        let Some(file) = self.file.as_mut() else {
            return;
        };
        if file.write_all(line.as_bytes()).is_err() {
            self.file = None;
            return;
        }
        self.written += line.len() as u64;
        if self.written > MAX_BYTES {
            // Moved aside while running too, not only at the next start.
            self.file = None;
            self.open();
        }
    }
}

fn without_numbers(body: &str) -> String {
    let mut key = String::with_capacity(body.len());
    let mut in_number = false;
    for character in body.chars() {
        if character.is_ascii_digit() {
            if !in_number {
                key.push('N');
            }
            in_number = true;
        } else {
            key.push(character);
            in_number = false;
        }
    }
    key
}

/// Lets through at most `per_second` entries a second from one source (a window's console), and says how many were
/// dropped: a source caught in a loop can't flood the log.
pub struct RateLimited<'a> {
    log: &'a DebugLog,
    per_second: u32,
    state: Mutex<(Option<Instant>, u32, u32)>,
}

impl<'a> RateLimited<'a> {
    pub fn new(log: &'a DebugLog, per_second: u32) -> Self {
        RateLimited {
            log,
            per_second,
            state: Mutex::new((None, 0, 0)),
        }
    }

    pub fn write(&self, level: Level, text: &str) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let (window_start, count, dropped) = &mut *state;
        if window_start.is_none_or(|start| start.elapsed() >= Duration::from_secs(1)) {
            if *dropped > 0 {
                self.log.warn(&format!(
                    "({dropped} more entries dropped in a second: too many)"
                ));
            }
            *window_start = Some(Instant::now());
            *count = 0;
            *dropped = 0;
        }
        *count += 1;
        if *count > self.per_second {
            *dropped += 1;
            return;
        }
        self.log.write(level, text);
    }
}

/// The `co-review` command's log: `[co-review] <message>` on stderr (stdout may belong to a protocol, MCP), and a line
/// in `logs/cli.log` naming the command (`co-review logs` shows them).
pub fn log(message: &str) {
    log_in(&installation::co_review_home(), message);
}

/// [`log`] into the logs of Co-Review home `home`.
pub fn log_in(home: &Path, message: &str) {
    eprintln!("[co-review] {message}");
    let command = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "start".to_string());
    let directory = installation::logs_directory(home);
    if fs::create_dir_all(&directory).is_err() {
        return;
    }
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join("cli.log"))
    {
        let line = format_line(Level::Info, &format!("{command}: {message}"));
        file.write_all(line.as_bytes()).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(directory: &Path) -> Vec<String> {
        fs::read_to_string(directory.join("test.log"))
            .unwrap_or_default()
            .lines()
            .map(|line| {
                line.splitn(4, ' ')
                    .nth(3)
                    .unwrap_or_default()
                    .trim()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn repeats_are_counted_with_the_latest_when_numbers_differ() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let log = DebugLog::new("test", directory.path());
        log.info("call 1 failed");
        log.info("call 2 failed");
        log.info("call 3 failed");
        log.warn("something else");
        drop(log);
        assert_eq!(
            lines(directory.path()),
            vec![
                "call 1 failed",
                "(the entry above came 2 more times, the last: call 3 failed)",
                "something else"
            ]
        );
    }

    #[test]
    fn entries_are_cut_and_multiline_entries_indented() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let log = DebugLog::new("test", directory.path());
        log.error(&format!("first\nsecond\n{}", "x".repeat(MAX_ENTRY)));
        drop(log);
        let text = fs::read_to_string(directory.path().join("test.log")).expect("log");
        assert!(text.contains(" ERROR first\n    second\n    xxx"));
        assert!(text.trim_end().ends_with("(13 more characters)"));
    }

    #[test]
    fn at_most_fifty_entries_a_second() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let log = DebugLog::new("test", directory.path());
        for index in 0..200 {
            log.info(&format!("entry {}", "a".repeat(index % 7 + 1)));
        }
        drop(log);
        let written = lines(directory.path());
        assert!(
            written.len() <= 2 * MAX_PER_SECOND as usize + 2,
            "{}",
            written.len()
        );
        assert!(
            written
                .iter()
                .any(|line| line.contains("more entries dropped in a second"))
        );
    }

    #[test]
    fn a_full_log_is_moved_aside() {
        let directory = tempfile::tempdir().expect("temporary directory");
        fs::write(
            directory.path().join("test.log"),
            vec![b'x'; MAX_BYTES as usize + 1],
        )
        .expect("write");
        let log = DebugLog::new("test", directory.path());
        log.info("fresh");
        drop(log);
        assert!(directory.path().join("test.1.log").exists());
        assert_eq!(lines(directory.path()), vec!["fresh"]);
    }
}
