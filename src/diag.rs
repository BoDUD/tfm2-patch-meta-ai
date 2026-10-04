//! `diag.log` (what the mod saw and did - send it when something looks wrong) and
//! `meta_table.txt` (the model's current numbers), both in the mod folder.
//!
//! Every function here swallows I/O errors: a read-only folder must never stop the mod.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Instant;

pub const LOG_FILE: &str = "diag.log";
pub const PREV_LOG_FILE: &str = "diag.prev.log";
pub const TABLE_FILE: &str = "meta_table.txt";

/// Lines written per game session; a stuck loop must not fill the disk.
const MAX_LINES: usize = 20_000;

struct Diag {
    dir: PathBuf,
    file: Option<File>,
    started: Instant,
    lines: usize,
    once: HashSet<String>,
}

static DIAG: Mutex<Option<Diag>> = Mutex::new(None);

fn lock() -> MutexGuard<'static, Option<Diag>> {
    DIAG.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Starts a new log for this game session; the previous session's log is kept as
/// `diag.prev.log`.
pub fn open(dir: &Path) {
    let path = dir.join(LOG_FILE);
    if path.exists() {
        let _ = fs::rename(&path, dir.join(PREV_LOG_FILE));
    }
    let file = OpenOptions::new().create(true).write(true).truncate(true).open(&path).ok();
    *lock() = Some(Diag {
        dir: dir.to_path_buf(),
        file,
        started: Instant::now(),
        lines: 0,
        once: HashSet::new(),
    });
}

pub fn log(msg: &str) {
    let mut guard = lock();
    let Some(diag) = guard.as_mut() else { return };
    if diag.lines >= MAX_LINES {
        return;
    }
    diag.lines += 1;
    let secs = diag.started.elapsed().as_secs_f64();
    if let Some(file) = diag.file.as_mut() {
        let line = if diag.lines == MAX_LINES {
            format!("[{secs:9.1}s] (log limit reached, nothing more this session)\n")
        } else {
            format!("[{secs:9.1}s] {msg}\n")
        };
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    }
}

/// Logs `msg` only the first time `key` is seen this session.
pub fn log_once(key: &str, msg: &str) {
    {
        let mut guard = lock();
        let Some(diag) = guard.as_mut() else { return };
        if !diag.once.insert(key.to_string()) {
            return;
        }
    }
    log(msg);
}

/// Forgets which `log_once` keys were used, so a new save logs its probes again.
pub fn reset_once() {
    if let Some(diag) = lock().as_mut() {
        diag.once.clear();
    }
}

/// Replaces `meta_table.txt` (written to a temporary file first, so a reader never sees
/// half a table).
pub fn write_table(text: &str) {
    let dir = match lock().as_ref() {
        Some(diag) => diag.dir.clone(),
        None => return,
    };
    let tmp = dir.join(format!("{TABLE_FILE}.tmp"));
    if fs::write(&tmp, text).is_ok() && fs::rename(&tmp, dir.join(TABLE_FILE)).is_err() {
        let _ = fs::write(dir.join(TABLE_FILE), text);
        let _ = fs::remove_file(&tmp);
    }
}

/// Shortens text for a log line (on a character boundary).
pub fn clip(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if text.chars().nth(max_chars).is_some() {
        out.push_str("...");
    }
    out
}
