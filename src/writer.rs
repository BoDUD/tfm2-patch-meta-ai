//! 后台写文件（2.15.7）。比赛线程和主线程只把要写的整份内容（或者要追加的一行）交过来就走，
//! 真正落盘在一条专门的线程里做。
//!
//! 以前平衡账本（几百 KB）在比赛线程里、拿着账本和比赛表的锁直接写：机械硬盘上一次打开文件卡了将近 10 秒，
//! 其余比赛线程和主线程都在等这两把锁，整个游戏停住（用户 11-28：「为什么过日变这么慢」）。
//!
//! - 整份写（[`replace`]）：同一个文件排着好几份时只写最新的一份；先写 `<文件名>.tmp` 再整体换上去，
//!   换不上就直接写（和 `diag::write_file` 原来的写法一样）。
//! - 追加（[`append`]）：按交过来的顺序一行一行追加。
//! - [`flush`]：等到这之前交过来的都写完。存档关闭、重新读同一本账本、游戏退出时用。
//! - 线程起不来就当场写（和以前一样慢，但不丢）。

use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

struct Queue {
    /// 每个文件最新的一份整份内容
    whole: BTreeMap<PathBuf, String>,
    /// 要追加的行，按交过来的顺序
    lines: VecDeque<(PathBuf, String)>,
    /// 交过来的次数、已经写完的那一次
    handed: u64,
    written: u64,
    started: bool,
    /// 线程起不来：之后都当场写
    failed: bool,
}

static QUEUE: Mutex<Queue> = Mutex::new(Queue {
    whole: BTreeMap::new(),
    lines: VecDeque::new(),
    handed: 0,
    written: 0,
    started: false,
    failed: false,
});
static WORK: Condvar = Condvar::new();
static DONE: Condvar = Condvar::new();

fn queue() -> MutexGuard<'static, Queue> {
    QUEUE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 线程在不在（不在就起一条）；起不来返回 false
fn running(q: &mut Queue) -> bool {
    if q.failed {
        return false;
    }
    if !q.started {
        match std::thread::Builder::new().name(format!("{}-writer", crate::MOD_ID)).spawn(run) {
            Ok(_) => q.started = true,
            Err(_) => {
                q.failed = true;
                return false;
            }
        }
    }
    true
}

/// 整份换掉一个文件（后台写，只写最新的一份）
pub fn replace(path: PathBuf, text: String) {
    let mut q = queue();
    if !running(&mut q) {
        drop(q);
        write_whole(&path, &text);
        return;
    }
    q.whole.insert(path, text);
    q.handed += 1;
    WORK.notify_one();
}

/// 往一个文件末尾追加一行（后台写，按顺序）
pub fn append(path: PathBuf, line: String) {
    let mut q = queue();
    if !running(&mut q) {
        drop(q);
        append_lines(&path, std::slice::from_ref(&line));
        return;
    }
    q.lines.push_back((path, line));
    q.handed += 1;
    WORK.notify_one();
}

/// 等到这之前交过来的都写完
pub fn flush() {
    let mut q = queue();
    let target = q.handed;
    while q.started && !q.failed && q.written < target {
        q = DONE.wait(q).unwrap_or_else(PoisonError::into_inner);
    }
}

fn run() {
    loop {
        let (whole, lines, upto) = {
            let mut q = queue();
            while q.whole.is_empty() && q.lines.is_empty() {
                q = WORK.wait(q).unwrap_or_else(PoisonError::into_inner);
            }
            (std::mem::take(&mut q.whole), std::mem::take(&mut q.lines), q.handed)
        };
        // 追加的行：同一个文件连着的几行一次写完
        let mut lines = lines.into_iter().peekable();
        while let Some((path, first)) = lines.next() {
            let mut batch = vec![first];
            while let Some((_, line)) = lines.next_if(|(p, _)| *p == path) {
                batch.push(line);
            }
            append_lines(&path, &batch);
        }
        for (path, text) in whole {
            write_whole(&path, &text);
        }
        let mut q = queue();
        q.written = q.written.max(upto);
        DONE.notify_all();
    }
}

fn write_whole(path: &Path, text: &str) {
    let Some(name) = path.file_name() else { return };
    let mut tmp_name = name.to_os_string();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    if std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::write(path, text);
        let _ = std::fs::remove_file(&tmp);
    }
}

fn append_lines(path: &Path, lines: &[String]) {
    let ok = std::fs::OpenOptions::new().create(true).append(true).open(path).and_then(|mut f| {
        let mut text = String::new();
        for line in lines {
            text.push_str(line);
            text.push('\n');
        }
        f.write_all(text.as_bytes())
    });
    if ok.is_err() {
        crate::diag::log_once("writer-append", &format!("[writer] could not append to {}", path.display()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pma_writer_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join(name)
    }

    #[test]
    fn the_newest_whole_text_is_on_disk_after_a_flush() {
        let file = temp("book.json");
        for i in 0..50 {
            replace(file.clone(), format!("{{\"round\":{i}}}"));
        }
        flush();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"round\":49}");
        assert!(!temp("book.json.tmp").exists(), "no temporary file is left behind");
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn appended_lines_keep_their_order() {
        let file = temp("rows.jsonl");
        let _ = std::fs::remove_file(&file);
        for i in 0..200 {
            append(file.clone(), format!("row {i}"));
        }
        flush();
        let text = std::fs::read_to_string(&file).unwrap();
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(rows.len(), 200);
        assert!(rows.iter().enumerate().all(|(i, r)| *r == format!("row {i}")));
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn many_threads_hand_over_without_waiting_on_the_disk() {
        let files: Vec<PathBuf> = (0..8).map(|i| temp(&format!("match{i}.json"))).collect();
        std::thread::scope(|s| {
            for (i, f) in files.iter().enumerate() {
                s.spawn(move || {
                    for k in 0..100 {
                        replace(f.clone(), format!("{i}:{k}"));
                    }
                });
            }
        });
        flush();
        for (i, f) in files.iter().enumerate() {
            assert_eq!(std::fs::read_to_string(f).unwrap(), format!("{i}:99"));
            let _ = std::fs::remove_file(f);
        }
    }
}
