//! The draft advice on a real save's matches (a `history/` file from the mod folder):
//!
//! `PMA_HISTORY=<mod folder>/history/teamN-xxxx.json cargo test --release --test save_board -- --ignored --nocapture`
//!
//! Prints the model's composition costs, then the best bans for an empty draft (3+2 phase one,
//! from both sides) and for a draft three picks in (phase two).

use patch_meta_ai::advisor::{self, Mastery, Team};
use patch_meta_ai::history::Names;
use patch_meta_ai::knowledge::{self, COMP_NAMES};
use patch_meta_ai::meta::{build, Inputs, Settings};

#[test]
#[ignore]
fn board_on_a_save() {
    let Ok(path) = std::env::var("PMA_HISTORY") else {
        eprintln!("PMA_HISTORY not set");
        return;
    };
    let text = std::fs::read_to_string(&path).expect("history file");
    let mut names = Names::default();
    let games: Vec<_> = patch_meta_ai::cache::decode(&text, &mut names).into_iter().map(|(_, g)| g).collect();
    let mut champions: Vec<String> = knowledge::get().positions.keys().cloned().collect();
    champions.sort();
    let current = games.iter().map(|g| g.version.clone()).max().unwrap_or_default();
    let meta = build(
        &Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: &current, warm: None },
        &Settings::default(),
    );
    eprintln!("{} games, patch {current}", games.len());
    for (i, e) in meta.comp.iter().enumerate() {
        eprintln!("  gap {:<17} {:+.3} +- {:.3}", COMP_NAMES[i], e.value, e.sd);
    }
    let name = |c: u16| meta.names.name(c).trim_start_matches("league_").to_string();
    let none = |_: u16| None;
    let id = |n: &str| meta.names.get(&format!("league_{n}")).expect(n);
    let show = |title: &str, ally: &[u16], enemy: &[u16], blue: bool| {
        let taken: Vec<u16> = ally.iter().chain(enemy).copied().collect();
        let offer: Vec<u16> = (0..meta.names.len() as u16).filter(|c| !taken.contains(c) && meta.profile(*c).is_some()).collect();
        let board = advisor::ban_board(
            &meta,
            Team::new(ally, &[]),
            Team::new(enemy, &[]),
            Mastery::FULL,
            &none,
            &offer,
            Some(blue),
        );
        eprintln!("\n{title}");
        for b in board.iter().take(8) {
            let t = &b.threat;
            eprintln!(
                "  {:<14} {:+.3}  deny {:.3} own {:.3}  {:?} lanes {} lone {}  str {:+.2} vs {:+.2} syn {:+.2} comp {:+.2}",
                name(b.champ), b.total, b.deny, b.own, t.role, b.lanes, b.irreplaceable, t.strength, t.counter, t.synergy, t.comp
            );
        }
    };
    show("phase 1, blue bans", &[], &[], true);
    show("phase 1, red bans", &[], &[], false);
    let blue = [id("ziggs"), id("sejuani"), id("caitlyn")];
    let red = [id("swain"), id("jax"), id("zeri")];
    show("phase 2, blue (ziggs sejuani caitlyn) bans vs red (swain jax zeri)", &blue, &red, true);
    show("phase 2, red bans", &red, &blue, false);

    // the lookahead as the screen runs it: five candidates played out from blue's first ban
    let offer: Vec<u16> = (0..meta.names.len() as u16).filter(|c| meta.profile(*c).is_some()).collect();
    let started = std::time::Instant::now();
    let pool = advisor::lookahead_pool(&meta, &offer, 28);
    let board = advisor::ban_board(&meta, Team::new(&[], &[]), Team::new(&[], &[]), Mastery::FULL, &none, &offer, Some(true));
    for b in board.iter().take(5) {
        let w = advisor::simulate(&meta, Team::new(&[], &[]), Team::new(&[], &[]), &pool, 0, b.champ, &none);
        eprintln!("  ban {:<14} -> blue {:.1}%", name(b.champ), w * 100.0);
    }
    eprintln!("lookahead, 5 candidates from the first ban: {:?}", started.elapsed());

    // which weight of League's knowledge predicts the newest matches best
    for w in [0.0, 0.5, 1.0, 2.0] {
        let set = Settings { lol_prior: w, ..Settings::default() };
        let inputs = Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: &current, warm: None };
        if let Some(bt) = patch_meta_ai::meta::backtest(&inputs, &set, (games.len() / 10).clamp(50, 300)) {
            eprintln!("lol_prior {w}: log-loss {:.4}, favourite won {:.1}% of {}", bt.log_loss / bt.games as f32, bt.accuracy * 100.0, bt.games);
        }
    }
}
