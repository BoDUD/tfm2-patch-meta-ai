//! What the mod costs the game, per frame: each part of the frame is timed, and once a minute
//! `diag.log` gets one `[perf]` line - average and worst time per part, and the worst whole
//! frame with the screen it was on. Timing is a pair of clock reads per part.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::diag;

/// The timed parts of a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// Reading records, fitting, tiers, report files.
    Client,
    /// The UI explorer (F9 / explore=on).
    Explore,
    /// The ban/pick screen overlay.
    Draft,
    /// The F8 panel.
    Panel,
    /// The Meta Analysis page.
    Page,
}

const PARTS: [Part; 5] = [Part::Client, Part::Explore, Part::Draft, Part::Panel, Part::Page];
const REPORT_EVERY: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Default)]
struct Stat {
    total: Duration,
    worst: Duration,
}

struct Perf {
    parts: [Stat; 5],
    frames: u32,
    frame: Duration,
    worst_frame: (Duration, String),
    since: Instant,
}

static PERF: Mutex<Option<Perf>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Perf) -> R) -> R {
    let mut guard = PERF.lock().unwrap_or_else(PoisonError::into_inner);
    let p = guard.get_or_insert_with(|| Perf {
        parts: [Stat::default(); 5],
        frames: 0,
        frame: Duration::ZERO,
        worst_frame: (Duration::ZERO, String::new()),
        since: Instant::now(),
    });
    f(p)
}

/// Runs `f` as part `part` of this frame.
pub fn time<R>(part: Part, f: impl FnOnce() -> R) -> R {
    let started = Instant::now();
    let out = f();
    let spent = started.elapsed();
    with(|p| {
        let s = &mut p.parts[part as usize];
        s.total += spent;
        s.worst = s.worst.max(spent);
        p.frame += spent;
    });
    out
}

/// Closes a frame (`screen` names where it was), and writes the minute's line when it is due.
pub fn end_frame(screen: &str) {
    let line = with(|p| {
        p.frames += 1;
        if p.frame > p.worst_frame.0 {
            p.worst_frame = (p.frame, screen.to_string());
        }
        p.frame = Duration::ZERO;
        if p.since.elapsed() < REPORT_EVERY {
            return None;
        }
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let n = p.frames.max(1) as f64;
        let parts: Vec<String> = PARTS
            .iter()
            .map(|part| {
                let s = p.parts[*part as usize];
                format!("{part:?} {:.3}/{:.1}", ms(s.total) / n, ms(s.worst))
            })
            .collect();
        let line = format!(
            "[perf] {} frames: {} (ms per frame avg/worst); worst frame {:.1} ms on {}",
            p.frames,
            parts.join(", "),
            ms(p.worst_frame.0),
            p.worst_frame.1
        );
        p.parts = [Stat::default(); 5];
        p.frames = 0;
        p.worst_frame = (Duration::ZERO, String::new());
        p.since = Instant::now();
        Some(line)
    });
    if let Some(line) = line {
        diag::log(&line);
    }
}
