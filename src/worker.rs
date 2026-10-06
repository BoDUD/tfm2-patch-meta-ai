//! Fitting the model off the game's frame loop. A fit takes tens of milliseconds on a big
//! save - several frames - so the client hands the games to one background thread and picks
//! the result up on a later frame. Only the newest job matters: jobs that queued up behind a
//! running fit are skipped. If the thread cannot be started the fit runs in the frame instead.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use crate::history::{Game, Names};
use crate::meta::{self, Backtest, Inputs, Meta, Settings};
use crate::patchnotes::PatchNote;

pub struct Job {
    /// Which rebuild this is (results of older ones are dropped).
    pub serial: u64,
    pub games: Vec<Game>,
    pub names: Names,
    pub champions: Vec<String>,
    pub notes: Vec<PatchNote>,
    pub current: String,
    pub settings: Settings,
    /// Also score the model on the newest games it was not fitted on.
    pub backtest: bool,
}

pub struct Done {
    pub serial: u64,
    pub meta: Meta,
    pub backtest: Option<Backtest>,
    pub millis: u128,
}

/// Holdout size for the backtest: a tenth of the matches, 50 to 300.
fn holdout(games: usize) -> usize {
    (games / 10).clamp(50, 300)
}

fn run(job: Job, warm: Option<&Meta>) -> Done {
    let started = std::time::Instant::now();
    let inputs = Inputs {
        games: &job.games,
        names: &job.names,
        champions: &job.champions,
        notes: &job.notes,
        current: &job.current,
        warm,
    };
    let meta = meta::build(&inputs, &job.settings);
    let backtest = if job.backtest {
        meta::backtest(&Inputs { warm: None, ..inputs }, &job.settings, holdout(job.games.len()))
    } else {
        None
    };
    Done { serial: job.serial, meta, backtest, millis: started.elapsed().as_millis() }
}

pub struct Worker {
    sender: Option<Sender<Job>>,
    done: Arc<Mutex<Option<Done>>>,
    /// The last fit, for a warm start when fitting in the frame.
    inline_warm: Option<Meta>,
}

impl Worker {
    /// A worker with its own thread (`threaded`), or one that fits in the caller's frame.
    pub fn new(threaded: bool) -> Self {
        let done = Arc::new(Mutex::new(None));
        let sender = if threaded { spawn(Arc::clone(&done)) } else { None };
        Self { sender, done, inline_warm: None }
    }

    pub fn submit(&mut self, job: Job) {
        let job = match &self.sender {
            Some(sender) => match sender.send(job) {
                Ok(()) => return,
                // the thread is gone: fit here from now on
                Err(err) => {
                    self.sender = None;
                    err.0
                }
            },
            None => job,
        };
        let done = run(job, self.inline_warm.as_ref());
        self.inline_warm = Some(done.meta.clone());
        *self.done.lock().unwrap_or_else(PoisonError::into_inner) = Some(done);
    }

    /// The newest finished fit, once.
    pub fn take(&mut self) -> Option<Done> {
        self.done.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

fn spawn(done: Arc<Mutex<Option<Done>>>) -> Option<Sender<Job>> {
    let (sender, receiver) = channel::<Job>();
    thread::Builder::new()
        .name(format!("{}-fit", crate::MOD_ID))
        .spawn(move || work(receiver, done))
        .ok()?;
    Some(sender)
}

fn work(receiver: Receiver<Job>, done: Arc<Mutex<Option<Done>>>) {
    let mut warm: Option<Meta> = None;
    while let Ok(mut job) = receiver.recv() {
        // only the newest waiting job
        while let Ok(newer) = receiver.try_recv() {
            job = newer;
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(job, warm.as_ref())));
        let Ok(result) = result else { continue };
        warm = Some(result.meta.clone());
        *done.lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::tests::{simulate, NAMES};

    fn job(serial: u64) -> Job {
        let (names, games) = simulate(400, "1.1", |n| if n == "c" { 0.5 } else { 0.0 }, 3);
        Job {
            serial,
            games,
            names,
            champions: NAMES.iter().map(|s| s.to_string()).collect(),
            notes: Vec::new(),
            current: "1.1".into(),
            settings: Settings::default(),
            backtest: true,
        }
    }

    #[test]
    fn fits_in_the_background_and_inline() {
        let mut threaded = Worker::new(true);
        threaded.submit(job(1));
        threaded.submit(job(2));
        let mut got = None;
        for _ in 0..500 {
            if let Some(done) = threaded.take() {
                got = Some(done);
                if got.as_ref().is_some_and(|d| d.serial == 2) {
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let done = got.expect("a fit came back");
        assert_eq!(done.serial, 2, "the newest job is the last result");
        assert!(done.meta.champion("c").unwrap().win_rate() > 0.52);
        assert!(done.backtest.is_some());

        let mut inline = Worker::new(false);
        inline.submit(job(7));
        assert_eq!(inline.take().map(|d| d.serial), Some(7));
        assert!(inline.take().is_none());
    }
}
