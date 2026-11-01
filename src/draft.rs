//! Ban/pick score hook. The game scores every candidate itself (lanes, its players' champion
//! pools); this adds what the meta model says about the candidate *in this draft* (`advisor`):
//!
//! - **pick**: its strength in the best lane still open, its synergy with the picks already on
//!   the team, its matchups against the enemy's picks, and a cost for one-sided damage;
//! - **ban**: how much worse the enemy's draft gets without it (`advisor::ban_board`: their
//!   best option in each lane they still have to fill, with and without it - an equally good
//!   substitute makes a ban worth little), less what it would take from our own options;
//! - **players** (game 0.6.4+, ABI level 10, which hands both line-ups over - athlete ids in
//!   lane-slot order): a pick also counts the mastery of the team's player in the lane the
//!   champion would play, a ban that of the enemy's player there (the whole line-up's average
//!   when the lane is not known), up to [`PLAYER_CAP`] and times `player_mastery` (0.5). The
//!   AI leans to its players' best champions and bans the enemy players' best ones.
//!
//! The value (log-odds) goes through `tanh(value / edge_scale)` so one extreme estimate cannot
//! swamp the game's own reasoning, then `pick_strength` / `ban_strength` scale it.
//!
//! The mastery is the model's own player-on-champion effect - how much more that player won on
//! that champion in this save than the champion's strength there and the player's own strength
//! say, shrunk towards 0 until enough games back it - never the game's proficiency number: the
//! game's own score already counts that, and adding it here would count it twice. The hook
//! reads no records (it cannot): the model is the snapshot the client publishes after every fit
//! (`shared`), and the line-ups come with each decision. Before the first fit of a save the hook
//! passes. Players the model does not know, a line-up the game does not hand over (games before
//! 0.6.4, a line-up not known yet) and `player_mastery = 0` leave the value exactly what it is
//! without line-ups.
//!
//! Position locks are another mod's job (Smart Position Lock): its "out of reach" score
//! (`Replace`, at priority 10000 - after this hook, [`PRIORITY`]) replaces whatever was added
//! here. Game 0.6.4 also adds a `Position` phase after an AI team's last pick, where a hook's
//! `decide_positions` may reassign the lanes and the last hook that decides wins: this hook
//! keeps the trait's default (`None`), so the lanes stay with the game or Smart Position Lock.
//! The score hooks never see that phase.
//!
//! `diag.log` gets the first decisions of a game session (`[draft]` lines): the line-ups the
//! game handed over, and for the best candidates their value, the players' part of it and what
//! was added to the game's score.
//!
//! **Coach override** ([`CoachHook`], `coach=on`): when the game's coach drafts for the player's
//! own team, a score nudge is not enough - another mod that decides the team's bans and picks
//! outright (Hell Draft AI with its `ally_draft_ai` on) wins over any score. So for the player's
//! team the coach hook decides outright too: the draft screen's own ranking when it was drawn
//! for this very draft (the panel's 禁 1 / 选, lookahead and position locks included,
//! `shared::CoachPlan`), else the best by the model's values among the champions that fit an
//! open lane. It runs at the highest priority, so of several deciding hooks it is the last.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use mod_api_stable::{LaneV1, StableDraftContext, StableDraftDecision, StableDraftHook};

use crate::advisor::PickValue;
use crate::history::Role;
use crate::shared::Snapshot;
use crate::{advisor, config, diag, shared};

pub struct MetaDraftHook;

/// Score hooks run in priority order, lower first. Smart Position Lock replaces the score of a
/// pick its locks rule out at priority 10000, after this hook, so its "out of reach" stands
/// whatever this hook added. Keep this below it.
pub const PRIORITY: i32 = 0;

impl StableDraftHook for MetaDraftHook {
    fn id(&self) -> String {
        format!("{}:draft", crate::MOD_ID)
    }

    fn priority(&self) -> i32 {
        PRIORITY
    }

    fn score_ban(&self, ctx: &StableDraftContext<'_>, candidate: usize, _base: f32) -> StableDraftDecision {
        score(ctx, candidate, true)
    }

    fn score_pick(&self, ctx: &StableDraftContext<'_>, candidate: usize, _base: f32) -> StableDraftDecision {
        score(ctx, candidate, false)
    }
}

/// The coach override: decides the player's own team's bans and picks outright (see the module
/// notes). A hook of its own, apart from [`MetaDraftHook`], so its priority can be the highest
/// while the score hook stays below the position lock's.
pub struct CoachHook;

/// Of several hooks that decide, the last one wins: this one goes last.
pub const COACH_PRIORITY: i32 = i32::MAX;

impl StableDraftHook for CoachHook {
    fn id(&self) -> String {
        format!("{}:coach", crate::MOD_ID)
    }

    fn priority(&self) -> i32 {
        COACH_PRIORITY
    }

    fn decide_ban(&self, ctx: &StableDraftContext<'_>) -> Option<usize> {
        coach_decide(ctx, true).or_else(|| ai_decide(ctx, true))
    }

    fn decide_pick(&self, ctx: &StableDraftContext<'_>) -> Option<usize> {
        coach_decide(ctx, false).or_else(|| ai_decide(ctx, false))
    }
}

/// AI decisions written to `diag.log` per game session besides those on screen (all of which
/// are written).
const AI_OFF_SCREEN_LINES: u32 = 6;
static AI_LOGGED: AtomicU32 = AtomicU32::new(0);

/// `ai_draft=on`: every AI team's ban or pick, decided outright the way the draft screen would
/// advise that team ([`advisor::plan_turn`]: the ban board, pick values, the 3+2 lookahead, the
/// fielded players' mastery) - among the champions that fit a lane it still has open. `None`
/// for the player's own team, with `ai_draft=off`, before the first fit, or with nothing on offer.
pub fn ai_decide(ctx: &StableDraftContext<'_>, ban: bool) -> Option<usize> {
    let cfg = config::get();
    let acting = ctx.ally_team();
    if !cfg.ai_draft || acting.is_none() || acting == shared::player_team() {
        return None;
    }
    let snapshot = shared::get()?;
    let model_id = |g: usize| ctx.champion_name(g).and_then(|n| snapshot.meta.names.get(n));
    let ids = |list: &[usize]| -> Vec<u16> { list.iter().filter_map(|g| model_id(*g)).collect() };
    let available: Vec<(usize, u16)> =
        ctx.available_champions().iter().filter_map(|g| Some((*g, model_id(*g)?))).collect();
    if available.is_empty() {
        return None;
    }
    let (ally, enemy) = (ids(ctx.ally_picks()), ids(ctx.enemy_picks()));
    let bans = ctx.ally_bans().len() + ctx.enemy_bans().len();
    let lineups = Lineups::from_game(ctx.ally_athletes(), ctx.enemy_athletes(), &ctx.slot_lanes());
    // both teams' habits (scouting), and this draft's own read of the champions (variety)
    let habits = shared::habits();
    let habits_of = |t: Option<usize>| habits.as_ref().and_then(|h| h.get(&u32::try_from(t?).ok()?));
    let habits = [habits_of(acting), habits_of(ctx.enemy_team())];
    let champ = if cfg.ai_variety > 0.0 {
        let progress = bans + ally.len() + enemy.len();
        let believed = variety::sampled(&snapshot, (acting, ctx.enemy_team()), progress, 1.0);
        varied_choice(&snapshot, &believed, cfg.ai_variety, &ally, &enemy, bans, ban, &available, &lineups, habits)?
    } else {
        ai_choice(&snapshot, &ally, &enemy, bans, ban, &available, &lineups, habits)?
    };
    let game_id = available.iter().find(|(_, m)| *m == champ).map(|(g, _)| *g)?;
    // the player's own match (the ban/pick screen is open, and the player's team is the other one)
    let live = LIVE.load(Ordering::Relaxed) && ctx.enemy_team().is_some() && ctx.enemy_team() == shared::player_team();
    if live {
        shared::note_ai_choice(ban, champ);
    }
    diag::log_once("ai-decides", "[ai] ai_draft=on: the AI teams' bans and picks follow the model (as the panel would advise them)");
    if live || AI_LOGGED.fetch_add(1, Ordering::Relaxed) < AI_OFF_SCREEN_LINES {
        diag::log(&format!(
            "[ai] {} {} {} vs {}{}: {}",
            team(&snapshot, acting),
            if ban { "ban" } else { "pick" },
            if ban { ctx.ally_bans().len() } else { ctx.ally_picks().len() } + 1,
            team(&snapshot, ctx.enemy_team()),
            if live { ", on screen" } else { "" },
            snapshot.meta.names.name(champ)
        ));
    }
    Some(game_id)
}

/// The AI's choice among `available` (game id, model id): the top of [`advisor::plan_turn`] for
/// it, its side read from where a 3+2 draft stands (no lookahead when that does not tell), both
/// teams' habits (`habits`: its own, the enemy's) counted as the panel counts them.
#[allow(clippy::too_many_arguments)]
pub fn ai_choice(
    snapshot: &Snapshot,
    ally: &[u16],
    enemy: &[u16],
    bans: usize,
    ban: bool,
    available: &[(usize, u16)],
    lineups: &Lineups,
    habits: [Option<&HashMap<u16, f32>>; 2],
) -> Option<u16> {
    ai_plan(snapshot, ally, enemy, bans, ban, available, lineups, habits, &[]).ranked(ban).first().copied()
}

/// `ai_variety`: the AI's choice by this draft's own read of the champions (`believed`, see
/// [`variety`]) - the first of its wishes that the model itself rates within `margin` win-rate
/// points of its own best, else the model's best. On a 3+2 turn the wishes are played out to the
/// end with the best few and compared by the win chance they end in; else by their value. So
/// the AI plays champions it is unsure of when they are close to the best, and never gives up
/// more than `margin` a turn.
#[allow(clippy::too_many_arguments)]
pub fn varied_choice(
    model: &Snapshot,
    believed: &Snapshot,
    margin: f32,
    ally: &[u16],
    enemy: &[u16],
    bans: usize,
    ban: bool,
    available: &[(usize, u16)],
    lineups: &Lineups,
    habits: [Option<&HashMap<u16, f32>>; 2],
) -> Option<u16> {
    let wished = ai_plan(believed, ally, enemy, bans, ban, available, lineups, habits, &[]).ranked(ban);
    let top: Vec<u16> = wished.iter().take(WISHES).copied().collect();
    let truth = ai_plan(model, ally, enemy, bans, ban, available, lineups, habits, &top);
    let best = truth.ranked(ban).first().copied()?;
    // in win-rate points: the win chance a played-out turn ends in, else the turn's own value
    let score: HashMap<u16, f32> = if truth.outlook.is_empty() {
        let values = if ban {
            truth.board.iter().map(|b| (b.champ, b.total)).collect::<Vec<_>>()
        } else {
            truth.picks.iter().map(|v| (v.champ, v.total)).collect()
        };
        values.into_iter().map(|(c, v)| (c, points(v))).collect()
    } else {
        truth.outlook.iter().map(|(c, w)| (*c, w * 100.0)).collect()
    };
    let floor = score.get(&best).copied().unwrap_or(0.0) - margin;
    Some(top.into_iter().find(|c| score.get(c).is_some_and(|v| *v >= floor)).unwrap_or(best))
}

/// The wishes of [`varied_choice`] it plays out (its read's best few).
const WISHES: usize = 3;

/// The AI's ranking of this ban or pick ([`advisor::plan_turn`]), `also` played out besides the
/// best few.
#[allow(clippy::too_many_arguments)]
fn ai_plan(
    snapshot: &Snapshot,
    ally: &[u16],
    enemy: &[u16],
    bans: usize,
    ban: bool,
    available: &[(usize, u16)],
    lineups: &Lineups,
    habits: [Option<&HashMap<u16, f32>>; 2],
    also: &[u16],
) -> advisor::TurnPlan {
    let meta = &snapshot.meta;
    let offer: Vec<u16> = available.iter().map(|(_, m)| *m).collect();
    let pickable = lane_fits(snapshot, ally, &offer, |t| t.ai);
    let damage = |c: u16| snapshot.damage_of(c);
    let turn = advisor::TurnInput {
        us: advisor::Team { habits: habits[0], ..advisor::Team::new(ally, &lineups.ally) },
        them: advisor::Team { habits: habits[1], ..advisor::Team::new(enemy, &lineups.enemy) },
        offer: &offer,
        pickable: &pickable,
        banning: ban,
        blue: advisor::side_from_order(bans, ally.len(), enemy.len(), ban),
        bans_made: bans,
        lookahead: true,
        conserve: 0.0,
        also,
    };
    advisor::plan_turn(meta, &turn, &damage)
}

/// `ai_draft`'s variety: each AI team drafts by its own read of the champions, drawn once per
/// draft - every champion's strength moved by its standard deviation times a standard normal
/// draw (Thompson sampling, times `weight`). A champion the model knows well barely moves; one
/// seen in few games may come out strong in this draft - so every champion gets its chances, the
/// AI is harder to predict, and the model gets games on all of them. [`varied_choice`] keeps
/// what it costs within `ai_variety` win-rate points a turn. The draw stays the same through one
/// draft (until the bans and picks start over).
pub mod variety {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, PoisonError};

    use crate::shared::Snapshot;

    /// Draws kept (team pairs), and believed snapshots kept (a copy of the model each).
    const DRAWS: usize = 256;
    const COPIES: usize = 6;

    struct Draw {
        /// Bans and picks made when last asked: fewer now means a new draft.
        progress: usize,
        z: Arc<HashMap<u16, f32>>,
        /// When it was last used (for dropping the oldest).
        used: u64,
    }

    #[derive(Default)]
    struct State {
        draws: HashMap<(Option<usize>, Option<usize>), Draw>,
        /// (model, draw, weight) -> the believed snapshot.
        copies: Vec<((usize, usize, u32), Arc<Snapshot>)>,
        clock: u64,
        seed: u64,
    }

    static STATE: Mutex<Option<State>> = Mutex::new(None);

    /// A standard normal draw from a 64-bit state (xorshift, Box-Muller).
    fn normal(state: &mut u64) -> f32 {
        let mut next = || {
            *state ^= *state << 13;
            *state ^= *state >> 7;
            *state ^= *state << 17;
            ((*state >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        };
        let (u, v) = (next(), next());
        ((-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()) as f32
    }

    /// The model as this team believes it in this draft (`base` itself when `weight` is 0).
    pub fn sampled(base: &Arc<Snapshot>, teams: (Option<usize>, Option<usize>), progress: usize, weight: f32) -> Arc<Snapshot> {
        if weight.is_nan() || weight <= 0.0 {
            return base.clone();
        }
        let mut guard = STATE.lock().unwrap_or_else(PoisonError::into_inner);
        let st = guard.get_or_insert_with(|| State {
            seed: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0x9e37, |d| d.as_nanos() as u64) | 1,
            ..State::default()
        });
        st.clock += 1;
        let clock = st.clock;
        let fresh = st.draws.get(&teams).is_none_or(|d| progress < d.progress);
        if fresh {
            if st.draws.len() >= DRAWS {
                if let Some(oldest) = st.draws.iter().min_by_key(|(_, d)| d.used).map(|(k, _)| *k) {
                    st.draws.remove(&oldest);
                }
            }
            let mut rng = st.seed ^ clock.wrapping_mul(0x9e37_79b9_7f4a_7c15);
            let z: HashMap<u16, f32> = base.meta.champions.iter().map(|c| (c.id, normal(&mut rng))).collect();
            st.seed = rng | 1;
            st.draws.insert(teams, Draw { progress, z: Arc::new(z), used: clock });
        }
        let draw = st.draws.get_mut(&teams).expect("just inserted");
        draw.progress = progress;
        draw.used = clock;
        let z = draw.z.clone();
        let key = (Arc::as_ptr(base) as usize, Arc::as_ptr(&z) as usize, weight.to_bits());
        if let Some((_, s)) = st.copies.iter().find(|(k, _)| *k == key) {
            return s.clone();
        }
        let mut believed = Snapshot::clone(base);
        for c in &mut believed.meta.champions {
            c.strength += weight * c.sd * z.get(&c.id).copied().unwrap_or(0.0);
        }
        let believed = Arc::new(believed);
        if st.copies.len() >= COPIES {
            st.copies.remove(0);
        }
        st.copies.push((key, believed.clone()));
        believed
    }

    /// Test support.
    #[doc(hidden)]
    pub fn reset() {
        *STATE.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

/// Where a coach decision came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoachSource {
    /// The draft screen's ranking for this draft.
    Panel,
    /// The model's values, worked out in the hook.
    Model,
}

/// The coach override's choice for the player's own team (the game's id), `None` for every
/// other team, with `coach=off`, before the first fit, or with nothing on offer.
pub fn coach_decide(ctx: &StableDraftContext<'_>, ban: bool) -> Option<usize> {
    let cfg = config::get();
    if !cfg.coach || ctx.ally_team().is_none() || ctx.ally_team() != shared::player_team() {
        return None;
    }
    let snapshot = shared::get()?;
    let model_id = |g: usize| ctx.champion_name(g).and_then(|n| snapshot.meta.names.get(n));
    let ids = |list: &[usize]| -> Vec<u16> { list.iter().filter_map(|g| model_id(*g)).collect() };
    let available: Vec<(usize, u16)> =
        ctx.available_champions().iter().filter_map(|g| Some((*g, model_id(*g)?))).collect();
    if available.is_empty() {
        return None;
    }
    let (ally, enemy) = (ids(ctx.ally_picks()), ids(ctx.enemy_picks()));
    let bans = ctx.ally_bans().len() + ctx.enemy_bans().len();
    let lineups = if cfg.player_mastery > 0.0 {
        Lineups::from_game(ctx.ally_athletes(), ctx.enemy_athletes(), &ctx.slot_lanes())
    } else {
        Lineups::default()
    };
    let (champ, source) = coach_choice(&snapshot, &ally, &enemy, bans, ban, &available, &lineups, cfg.player_mastery)?;
    let game_id = available.iter().find(|(_, m)| *m == champ).map(|(g, _)| *g)?;
    shared::note_coach_choice(ban, champ);
    diag::log_once("coach-decides", "[coach] the coach override decides the player's team's bans and picks (coach=on)");
    diag::log(&format!(
        "[coach] {} {} {}: {} ({})",
        team(&snapshot, ctx.ally_team()),
        if ban { "ban" } else { "pick" },
        if ban { ctx.ally_bans().len() } else { ctx.ally_picks().len() } + 1,
        snapshot.meta.names.name(champ),
        match source {
            CoachSource::Panel => "as on the panel",
            CoachSource::Model => "the model's best",
        }
    ));
    Some(game_id)
}

/// The coach's choice among `available` (game id, model id): the panel's ranking when it was
/// drawn for this draft (sorted picks, ban count and phase alike), else the model's best value -
/// for a pick among the champions that fit a lane the team still has open and that no position
/// lock holds.
#[allow(clippy::too_many_arguments)]
pub fn coach_choice(
    snapshot: &Snapshot,
    ally: &[u16],
    enemy: &[u16],
    bans: usize,
    ban: bool,
    available: &[(usize, u16)],
    lineups: &Lineups,
    player_weight: f32,
) -> Option<(u16, CoachSource)> {
    let on_offer = |c: u16| available.iter().any(|(_, m)| *m == c);
    let plan = shared::coach_plan();
    let (mut a, mut e) = (ally.to_vec(), enemy.to_vec());
    a.sort_unstable();
    e.sort_unstable();
    if let Some(p) = plan.as_ref().filter(|p| p.ban == ban && p.bans == bans && p.ally == a && p.enemy == e) {
        if let Some(c) = p.ranked.iter().copied().find(|c| on_offer(*c)) {
            return Some((c, CoachSource::Panel));
        }
    }
    let held: &[u16] = plan.as_ref().map_or(&[], |p| p.held.as_slice());
    let offer: Vec<u16> = available.iter().map(|(_, m)| *m).collect();
    let draft = Draft { ally, enemy, offer: &offer, lineups };
    let fits = lane_fits(snapshot, ally, &offer, |t| t.player);
    offer
        .iter()
        .copied()
        .filter(|c| ban || (!held.contains(c) && fits.contains(c)))
        .map(|c| (c, value_with(snapshot, c, &draft, ban, player_weight).total))
        .max_by(|x, y| x.1.partial_cmp(&y.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(c, _)| (c, CoachSource::Model))
}

/// The champions of `offer` a team that picked `ally` may pick: by Smart Position Lock's own rule
/// and table when it holds this team's picks (`holds`: its AI or player switch) - a deciding hook
/// goes past its score - else those that fit a lane the model sees still open.
fn lane_fits(snapshot: &Snapshot, ally: &[u16], offer: &[u16], holds: impl Fn(&crate::lock::Table) -> bool) -> Vec<u16> {
    let names = &snapshot.meta.names;
    match crate::lock::get().filter(|t| holds(t)) {
        Some(t) => {
            let team: Vec<&str> = ally.iter().map(|c| names.name(*c)).collect();
            let named: Vec<(u16, &str)> = offer.iter().map(|c| (*c, names.name(*c))).collect();
            t.pickable(&team, &named)
        }
        None => {
            let open = advisor::open_lanes(&snapshot.meta, ally);
            let fit: Vec<u16> = offer.iter().copied().filter(|c| advisor::fits_lanes(&snapshot.meta, *c, &open)).collect();
            if fit.is_empty() { offer.to_vec() } else { fit }
        }
    }
}

/// A team's fielded players and the lane each plays (`None`: not known).
pub type Lineup = Vec<(u32, Option<Role>)>;

/// Both line-ups of a draft: the deciding team's and its opponent's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lineups {
    pub ally: Lineup,
    pub enemy: Lineup,
}

impl Lineups {
    /// From what the game hands over: athlete ids in lane-slot order, and the lanes of the
    /// deciding team's slots when the context lists them (the `Position` phase only). A full
    /// line-up of five plays top to support; any other count's lanes are not known.
    pub fn from_game(ally: &[usize], enemy: &[usize], ally_slot_lanes: &[LaneV1]) -> Self {
        Self { ally: lineup(ally, ally_slot_lanes), enemy: lineup(enemy, &[]) }
    }

    pub fn is_empty(&self) -> bool {
        self.ally.is_empty() && self.enemy.is_empty()
    }
}

fn lineup(athletes: &[usize], slot_lanes: &[LaneV1]) -> Lineup {
    let lanes: Vec<Option<Role>> = if !slot_lanes.is_empty() && slot_lanes.len() == athletes.len() {
        slot_lanes.iter().map(|lane| Role::ALL.get(lane.code() as usize).copied()).collect()
    } else if athletes.len() == Role::ALL.len() {
        Role::ALL.into_iter().map(Some).collect()
    } else {
        vec![None; athletes.len()]
    };
    athletes.iter().zip(lanes).filter_map(|(a, lane)| Some((u32::try_from(*a).ok()?, lane))).collect()
}

/// The player's own team's draft under coach delegation counts the model this many times as
/// strongly as an AI team's.
pub const COACH_STRENGTH: f32 = 2.0;

/// Whether the hook acts on a team's decision, and how strongly (a factor on `pick_strength` /
/// `ban_strength`): every AI team with `ban_pick` on; the player's own team (it only drafts
/// through the hooks when the game's coach drafts for it) with `coach` on, at
/// [`COACH_STRENGTH`] - whether or not `ban_pick` is on.
pub fn acting_strength(ban_pick: bool, coach: bool, team: Option<usize>, player_team: Option<usize>) -> Option<f32> {
    let own = team.is_some() && team == player_team;
    if own && coach {
        Some(COACH_STRENGTH)
    } else if ban_pick {
        Some(1.0)
    } else {
        None
    }
}

fn score(ctx: &StableDraftContext<'_>, candidate: usize, ban: bool) -> StableDraftDecision {
    let cfg = config::get();
    let Some(factor) = acting_strength(cfg.ban_pick_on(), cfg.coach, ctx.ally_team(), shared::player_team()) else {
        return StableDraftDecision::Pass;
    };
    if factor > 1.0 {
        diag::log_once("draft-coach", "[draft] coach delegation: the player's team drafts by the model (coach=on)");
    }
    let Some(snapshot) = shared::get() else {
        diag::log_once(
            "draft-cold",
            "[draft] the AI drafted with no model ready (no save open, or its matches not read yet): left to the game",
        );
        return StableDraftDecision::Pass;
    };
    let ids = |list: &[usize]| -> Vec<u16> {
        list.iter().filter_map(|id| ctx.champion_name(*id)).filter_map(|n| snapshot.meta.names.get(n)).collect()
    };
    let Some(cand) = ctx.champion_name(candidate).and_then(|n| snapshot.meta.names.get(n)) else {
        return StableDraftDecision::Pass;
    };
    let ally = ids(ctx.ally_picks());
    let enemy = ids(ctx.enemy_picks());
    let offer = offer_ids(&snapshot, ctx.available_champions(), &ids);
    let lineups = if cfg.player_mastery > 0.0 {
        Lineups::from_game(ctx.ally_athletes(), ctx.enemy_athletes(), &ctx.slot_lanes())
    } else {
        Lineups::default()
    };
    let draft = Draft { ally: &ally, enemy: &enemy, offer: &offer, lineups: &lineups };
    let v = value_with(&snapshot, cand, &draft, ban, cfg.player_mastery);
    let strength = if ban { cfg.ban_strength } else { cfg.pick_strength } * factor;
    let add = amount(v.total, cfg.edge_scale, strength);
    note(ctx, &snapshot, &lineups, ban, &v, add, cfg.player_mastery);
    decision(add)
}

/// The model it was for, the game's ids, and the model's ids.
type Offer = (usize, Vec<usize>, Vec<u16>);

thread_local! {
    /// The offer of the decision in progress, as the model's ids: the game asks about every
    /// candidate of one decision in a row, with the same offer.
    static OFFER: RefCell<Option<Offer>> = const { RefCell::new(None) };
}

fn offer_ids(snapshot: &Arc<Snapshot>, available: &[usize], ids: &dyn Fn(&[usize]) -> Vec<u16>) -> Vec<u16> {
    OFFER.with(|cell| {
        let mut cache = cell.borrow_mut();
        let model = Arc::as_ptr(snapshot) as usize;
        if let Some((m, raw, out)) = cache.as_ref() {
            if *m == model && raw.as_slice() == available {
                return out.clone();
            }
        }
        let out = ids(available);
        *cache = Some((model, available.to_vec(), out.clone()));
        out
    })
}

thread_local! {
    /// The open lanes of the last team asked about: the game scores every candidate of one
    /// decision in a row, with the same picks (hooks may run on several threads).
    static OPEN: RefCell<Option<(Vec<u16>, Vec<Role>)>> = const { RefCell::new(None) };
}

fn open_lanes(snapshot: &Snapshot, picks: &[u16]) -> Vec<Role> {
    OPEN.with(|cell| {
        let mut cache = cell.borrow_mut();
        if let Some((key, open)) = cache.as_ref() {
            if key == picks {
                return open.clone();
            }
        }
        let open = advisor::open_lanes(&snapshot.meta, picks);
        *cache = Some((picks.to_vec(), open.clone()));
        open
    })
}

/// One decision in the model's ids.
pub struct Draft<'a> {
    pub ally: &'a [u16],
    pub enemy: &'a [u16],
    /// What is still on offer (how counterable a pick is while the other side has picks left).
    pub offer: &'a [u16],
    pub lineups: &'a Lineups,
}

/// A ban's value is what it takes from the enemy's best option - a difference between options,
/// smaller than a pick's own value: this brings it to the same scale before `edge_scale`.
pub const BAN_SCALE: f32 = 2.0;

thread_local! {
    /// The ban board of the decision in progress: the game asks about every candidate of one
    /// decision in a row, with the same draft.
    static BOARD: RefCell<Option<(u64, Board)>> = const { RefCell::new(None) };
}

/// A ban board by champion.
type Board = Arc<HashMap<u16, advisor::BanValue>>;

/// The deciding team's ban board for this draft (worked out once per decision).
fn board(snapshot: &Snapshot, draft: &Draft<'_>, mastery: advisor::Mastery) -> Board {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (snapshot as *const Snapshot as usize, draft.ally, draft.enemy, draft.offer, &draft.lineups.ally, &draft.lineups.enemy).hash(&mut h);
    mastery.weight.to_bits().hash(&mut h);
    let key = h.finish();
    BOARD.with(|cell| {
        let mut cache = cell.borrow_mut();
        if let Some((k, board)) = cache.as_ref() {
            if *k == key {
                return board.clone();
            }
        }
        let damage = |c: u16| snapshot.damage_of(c);
        let us = advisor::Team::new(draft.ally, &draft.lineups.ally);
        let them = advisor::Team::new(draft.enemy, &draft.lineups.enemy);
        // the AI's side is not handed over: who picks first is known only from the pick counts
        let list = advisor::ban_board(&snapshot.meta, us, them, mastery, &damage, draft.offer, None);
        let board = Arc::new(list.into_iter().map(|b| (b.champ, b)).collect::<HashMap<_, _>>());
        *cache = Some((key, board.clone()));
        board
    })
}

/// A fielded player's mastery counts up to this much (log-odds, about 12 win-rate points)
/// before `player_mastery` weighs it: at the default 0.5 the players' part of a value stays
/// within +-0.25 (about 6 points), whatever the model makes of a few lopsided games.
pub const PLAYER_CAP: f32 = 0.5;

/// The model's value (log-odds) of picking or banning `cand` in this draft, the fielded
/// players' mastery counted at `player_weight`: a pick's for the deciding team's player in the
/// lane it would play, a ban's for the enemy's.
pub fn value_with(snapshot: &Snapshot, cand: u16, draft: &Draft<'_>, ban: bool, player_weight: f32) -> PickValue {
    let damage = |c: u16| snapshot.damage_of(c);
    let meta = &snapshot.meta;
    let (ally, enemy) = (draft.ally, draft.enemy);
    let mastery = advisor::Mastery { weight: player_weight, cap: PLAYER_CAP };
    if ban {
        if let Some(b) = board(snapshot, draft, mastery).get(&cand) {
            return PickValue { total: b.total * BAN_SCALE, ..b.threat.clone() };
        }
        // not on offer (the game asked anyway): as the enemy's pick, weighted by presence
        let open = open_lanes(snapshot, enemy);
        let v = advisor::ban_value_weighted(meta, cand, ally, enemy, &draft.lineups.enemy, mastery, &damage, &open);
        advisor::with_exposure(meta, v, 5usize.saturating_sub(ally.len()), draft.offer)
    } else {
        let open = open_lanes(snapshot, ally);
        let v = advisor::pick_value_weighted(meta, cand, ally, enemy, &draft.lineups.ally, mastery, &damage, &open);
        advisor::with_exposure(meta, v, 5usize.saturating_sub(enemy.len()), draft.offer)
    }
}

/// [`value_with`] without line-ups: the value before game 0.6.4.
pub fn value(snapshot: &Snapshot, cand: u16, ally: &[u16], enemy: &[u16], ban: bool, offer: &[u16]) -> f32 {
    let none = Lineups::default();
    value_with(snapshot, cand, &Draft { ally, enemy, offer, lineups: &none }, ban, 0.0).total
}

/// What is added to the game's score.
pub fn amount(value: f32, edge_scale: f32, strength: f32) -> f32 {
    (value / edge_scale).tanh() * strength
}

pub fn decision(v: f32) -> StableDraftDecision {
    if v.is_finite() && v.abs() > 1e-4 {
        StableDraftDecision::Add(v)
    } else {
        StableDraftDecision::Pass
    }
}

// ---------------------------------------------------------------------------------- diag.log

/// The ban/pick screen is open (set by `ui`): the drafts now are the player's own match.
static LIVE: AtomicBool = AtomicBool::new(false);
/// Decisions written out per game session: in the player's own matches (the ban/pick screen is
/// open), and in the drafts the game simulates off screen (the other matches of a match day).
const LIVE_DECISIONS: u32 = 24;
const OFF_SCREEN_DECISIONS: u32 = 6;
static LIVE_LOGGED: AtomicU32 = AtomicU32::new(0);
static OFF_SCREEN_LOGGED: AtomicU32 = AtomicU32::new(0);
/// Line-up pairs written out (each once) per game session, at most [`LINEUP_LINES`].
const LINEUP_LINES: usize = 40;
static LINEUPS_SEEN: Mutex<Vec<u64>> = Mutex::new(Vec::new());
/// Candidates shown per decision.
const SHOWN: usize = 3;

/// The ban/pick screen opened (`true`) or closed.
pub fn set_live(live: bool) {
    LIVE.store(live, Ordering::Relaxed);
}

/// The decision in progress on this thread (the game asks about every candidate of one decision
/// in a row; hooks may run on several threads).
struct Pending {
    key: u64,
    /// Written to `diag.log` (else only followed, so the next decision is told apart).
    logged: bool,
    /// Candidates on offer: the decision is written out once each was asked about.
    expected: usize,
    seen: usize,
    head: String,
    /// (value, players' part, text) of each candidate.
    rows: Vec<(f32, f32, String)>,
}

thread_local! {
    static PENDING: RefCell<Option<Pending>> = const { RefCell::new(None) };
}

fn decision_key(ctx: &StableDraftContext<'_>, ban: bool) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (ban, ctx.ally_team(), ctx.enemy_team(), ctx.ally_picks(), ctx.enemy_picks()).hash(&mut h);
    (ctx.ally_bans(), ctx.enemy_bans(), ctx.available_champions().len()).hash(&mut h);
    h.finish()
}

/// One scored candidate, for `diag.log`.
fn note(ctx: &StableDraftContext<'_>, snapshot: &Snapshot, lineups: &Lineups, ban: bool, v: &PickValue, add: f32, weight: f32) {
    let key = decision_key(ctx, ban);
    PENDING.with(|cell| {
        let mut pending = cell.borrow_mut();
        if pending.as_ref().is_none_or(|p| p.key != key) {
            if let Some(done) = pending.as_mut() {
                flush(done);
            }
            *pending = Some(start(ctx, snapshot, lineups, ban, key, weight));
        }
        let Some(p) = pending.as_mut() else { return };
        p.seen += 1;
        if p.logged {
            p.rows.push((v.total, v.mastery, row(snapshot, v, add, ban)));
            if p.seen >= p.expected {
                flush(p);
            }
        }
    });
}

/// A new decision: written out while this session's share is not used up.
fn start(ctx: &StableDraftContext<'_>, snapshot: &Snapshot, lineups: &Lineups, ban: bool, key: u64, weight: f32) -> Pending {
    let live = LIVE.load(Ordering::Relaxed);
    let (count, cap) = if live { (&LIVE_LOGGED, LIVE_DECISIONS) } else { (&OFF_SCREEN_LOGGED, OFF_SCREEN_DECISIONS) };
    let logged = count.load(Ordering::Relaxed) < cap && count.fetch_add(1, Ordering::Relaxed) < cap;
    let mut head = String::new();
    if logged {
        let number = if ban { ctx.ally_bans().len() } else { ctx.ally_picks().len() } + 1;
        head = format!(
            "[draft] {} {} {number} vs {}{}",
            team(snapshot, ctx.ally_team()),
            if ban { "ban" } else { "pick" },
            team(snapshot, ctx.enemy_team()),
            if live { ", on screen" } else { "" }
        );
        lineup_line(ctx, snapshot, lineups, live, weight);
    }
    Pending { key, logged, expected: ctx.available_champions().len().max(1), seen: 0, head, rows: Vec::new() }
}

/// The decision's best candidates (and the one the players moved most), written out once.
fn flush(p: &mut Pending) {
    if !p.logged || p.rows.is_empty() {
        p.logged = false;
        return;
    }
    p.logged = false;
    let mut rows = std::mem::take(&mut p.rows);
    rows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut shown: Vec<String> = rows.iter().take(SHOWN).map(|r| r.2.clone()).collect();
    let moved = rows
        .iter()
        .enumerate()
        .max_by(|a, b| a.1 .1.abs().partial_cmp(&b.1 .1.abs()).unwrap_or(std::cmp::Ordering::Equal));
    if let Some((i, r)) = moved {
        if i >= SHOWN && r.1 != 0.0 {
            shown.push(format!("players moved most: {}", r.2));
        }
    }
    diag::log(&format!("{} ({} asked): {}", p.head, rows.len(), shown.join("; ")));
}

fn points(v: f32) -> f32 {
    (crate::glm::sigmoid(v) - 0.5) * 100.0
}

/// `ahri +4.1 (players +1.2: Mid #583 Faker) -> +0.71`: the value and the players' part in
/// win-rate points, and what was added to the game's score.
fn row(snapshot: &Snapshot, v: &PickValue, add: f32, ban: bool) -> String {
    let whose = if ban { "their players" } else { "players" };
    let players = if v.mastery == 0.0 {
        format!("{whose} 0")
    } else {
        match v.player {
            Some(a) => format!(
                "{whose} {:+.1}: {} {}",
                points(v.mastery),
                v.role.map_or("?", |r| r.name()),
                athlete(snapshot, a)
            ),
            None => format!("{whose} {:+.1}: line-up", points(v.mastery)),
        }
    };
    format!("{} {:+.1} ({players}) -> {add:+.2}", snapshot.meta.names.name(v.champ), points(v.total))
}

fn athlete(snapshot: &Snapshot, a: u32) -> String {
    match snapshot.athlete_names.get(&a) {
        Some(name) => format!("#{a} {name}"),
        None => format!("#{a}"),
    }
}

fn team(snapshot: &Snapshot, id: Option<usize>) -> String {
    match id {
        Some(id) => match u32::try_from(id).ok().and_then(|t| snapshot.team_names.get(&t)) {
            Some(name) => format!("{name} (#{id})"),
            None => format!("team #{id}"),
        },
        None => "team ?".to_string(),
    }
}

/// The line-ups of a logged decision, each pair once per session: every player with their lane,
/// and how many of them the model saw play that lane most (a check of the lane-slot order).
fn lineup_line(ctx: &StableDraftContext<'_>, snapshot: &Snapshot, lineups: &Lineups, live: bool, weight: f32) {
    if weight <= 0.0 {
        diag::log_once("draft-lineups-off", "[draft] player_mastery=0: the line-ups are not used");
        return;
    }
    if lineups.is_empty() {
        diag::log_once(
            "draft-no-lineups",
            "[draft] the game hands over no line-ups (a game before 0.6.4, or a line-up not known yet): \
             picks and bans without the players' mastery",
        );
        return;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (ctx.ally_team(), ctx.enemy_team(), lineups.ally.as_slice(), lineups.enemy.as_slice()).hash(&mut h);
    let key = h.finish();
    {
        let mut seen = LINEUPS_SEEN.lock().unwrap_or_else(PoisonError::into_inner);
        if seen.contains(&key) || seen.len() >= LINEUP_LINES {
            return;
        }
        seen.push(key);
    }
    let show = |lineup: &Lineup| -> String {
        let (mut fits, mut known) = (0, 0);
        let players: Vec<String> = lineup
            .iter()
            .map(|(a, lane)| {
                if let (Some(lane), Some(usual)) = (lane, snapshot.meta.usual_lane(*a)) {
                    known += 1;
                    fits += (*lane == usual) as u32;
                }
                format!("{} {}", lane.map_or("?", |r| r.name()), athlete(snapshot, *a))
            })
            .collect();
        let check = if known > 0 { format!(", {fits}/{known} in their usual lane") } else { String::new() };
        format!("[{}]{check}", players.join(", "))
    };
    diag::log(&format!(
        "[draft] line-ups{}: {} {} vs {} {}",
        if live { " on screen" } else { "" },
        team(snapshot, ctx.ally_team()),
        show(&lineups.ally),
        team(snapshot, ctx.enemy_team()),
        show(&lineups.enemy)
    ));
}

/// Test support: a new game session's logging.
#[doc(hidden)]
pub fn reset_for_tests() {
    LIVE.store(false, Ordering::Relaxed);
    LIVE_LOGGED.store(0, Ordering::Relaxed);
    AI_LOGGED.store(0, Ordering::Relaxed);
    OFF_SCREEN_LOGGED.store(0, Ordering::Relaxed);
    LINEUPS_SEEN.lock().unwrap_or_else(PoisonError::into_inner).clear();
    PENDING.with(|cell| *cell.borrow_mut() = None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::tests::{simulate, NAMES};
    use crate::meta::{build, Effect, Inputs, Settings, Tally};
    use std::collections::HashMap;

    #[test]
    fn coach_delegation_drafts_for_the_player_only() {
        // ban_pick off: only the player's own team (coach delegation)
        assert_eq!(acting_strength(false, true, Some(19), Some(19)), Some(COACH_STRENGTH));
        assert_eq!(acting_strength(false, true, Some(3), Some(19)), None, "other teams untouched");
        assert_eq!(acting_strength(false, false, Some(19), Some(19)), None, "coach=off");
        assert_eq!(acting_strength(false, true, None, None), None, "teams not known");
        // ban_pick on: every team, the player's at the coach strength
        assert_eq!(acting_strength(true, true, Some(3), Some(19)), Some(1.0));
        assert_eq!(acting_strength(true, false, Some(19), Some(19)), Some(1.0));
    }

    #[test]
    fn amounts_are_bounded() {
        assert!((amount(0.25, 0.5, 1.0) - 0.5f32.tanh()).abs() < 1e-6);
        assert!(amount(10.0, 0.5, 0.8) <= 0.8 + 1e-6);
        assert!(amount(-0.3, 0.5, 1.0) < 0.0);
    }

    #[test]
    fn decisions() {
        assert!(matches!(decision(0.3), StableDraftDecision::Add(v) if (v - 0.3).abs() < 1e-6));
        assert!(matches!(decision(0.0), StableDraftDecision::Pass));
        assert!(matches!(decision(f32::NAN), StableDraftDecision::Pass));
    }

    #[test]
    fn runs_before_the_position_lock() {
        // Smart Position Lock replaces the scores it rules out at priority 10000: after this hook
        assert!(MetaDraftHook.priority() < 10_000);
    }

    #[test]
    fn line_ups_from_the_game() {
        let five = Lineups::from_game(&[11, 12, 13, 14, 15], &[21, 22, 23, 24, 25], &[]);
        assert_eq!(five.ally, Role::ALL.iter().zip(11u32..).map(|(r, a)| (a, Some(*r))).collect::<Lineup>());
        assert_eq!(five.enemy[4], (25, Some(Role::Support)));
        // the slots' own lanes when the game lists them (deciding team only)
        let seated = Lineups::from_game(
            &[11, 12, 13, 14, 15],
            &[21, 22, 23, 24, 25],
            &[LaneV1::Support, LaneV1::Bottom, LaneV1::Mid, LaneV1::Jungle, LaneV1::Top],
        );
        assert_eq!(seated.ally[0], (11, Some(Role::Support)));
        assert_eq!(seated.enemy[0], (21, Some(Role::Top)));
        // not five: lanes unknown; nothing: nothing
        let three = Lineups::from_game(&[11, 12, 13], &[], &[]);
        assert_eq!(three.ally, vec![(11, None), (12, None), (13, None)]);
        assert!(three.enemy.is_empty() && !three.is_empty());
        assert!(Lineups::from_game(&[], &[], &[]).is_empty());
    }

    fn snapshot() -> Snapshot {
        let (names, games) = simulate(2000, "1.1", |n| if n == "c" { 0.4 } else { 0.0 }, 5);
        let champions: Vec<String> = NAMES.iter().map(|s| s.to_string()).collect();
        let meta = build(
            &Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: "1.1", warm: None, stats: None },
            &Settings::default(),
        );
        Snapshot::new(meta, HashMap::new())
    }

    /// Athlete 501 (unknown to the simulation) masters "f".
    fn with_mastery(mut s: Snapshot, value: f32) -> (Snapshot, u16) {
        let f = s.meta.names.get("f").unwrap();
        s.meta.mastery.insert((501, f), Effect { value, sd: 0.1, tally: Tally { games: 40, wins: 30 } });
        (s, f)
    }

    /// A full line-up with `who` in `lane` and players the model does not know elsewhere.
    fn lineup_with(who: u32, lane: Role) -> Lineup {
        Role::ALL.iter().map(|r| (if *r == lane { who } else { 900 + r.index() as u32 }, Some(*r))).collect()
    }

    #[test]
    fn fielded_players_count_for_picks_and_bans() {
        let (s, f) = with_mastery(snapshot(), 0.5);
        let offer: Vec<u16> = (0..12).collect();
        let (ally, enemy) = (vec![s.meta.names.get("a").unwrap()], vec![s.meta.names.get("b").unwrap()]);
        let plain = |ban: bool| value(&s, f, &ally, &enemy, ban, &offer);
        let with = |lineups: &Lineups, ban: bool, w: f32| {
            value_with(&s, f, &Draft { ally: &ally, enemy: &enemy, offer: &offer, lineups }, ban, w)
        };
        // the lane "f" would play for the AI team, and for its opponent
        let none = Lineups::default();
        let pick_lane = with(&none, false, 0.5).role.unwrap();
        let ban_lane = with(&none, true, 0.5).role.unwrap();

        // pick: the team's own player in that lane counts, at the weight
        let ours = Lineups { ally: lineup_with(501, pick_lane), enemy: Vec::new() };
        let v = with(&ours, false, 0.5);
        assert_eq!(v.player, Some(501));
        assert!((v.mastery - 0.25).abs() < 1e-6 && (v.total - plain(false) - 0.25).abs() < 1e-5, "{v:?}");
        // ... not the enemy's, and only in that lane
        let theirs = Lineups { ally: Vec::new(), enemy: lineup_with(501, ban_lane) };
        assert_eq!(with(&theirs, false, 0.5).total, plain(false));
        let elsewhere = Role::ALL.into_iter().find(|r| *r != pick_lane).unwrap();
        assert_eq!(with(&Lineups { ally: lineup_with(501, elsewhere), enemy: Vec::new() }, false, 0.5).total, plain(false));

        // ban: the enemy's player in the lane it would play for them makes it a better ban
        let b = with(&theirs, true, 0.5);
        assert_eq!(b.player, Some(501));
        assert!(b.total > plain(true), "{b:?} vs {}", plain(true));
        // ... and our own player on it makes it a worse one: we would rather take it ourselves
        assert!(with(&ours, true, 0.5).total <= plain(true), "our own player does not make it a ban");

        // a line-up whose lanes are not known: the line-up as a whole
        let three = Lineups { ally: vec![(501, None), (902, None), (903, None)], enemy: Vec::new() };
        let v = with(&three, false, 0.5);
        assert!(v.player.is_none() && (v.mastery - 0.5 / 3.0 * 0.5).abs() < 1e-6, "{v:?}");
    }

    #[test]
    fn unknown_players_and_no_line_ups_change_nothing() {
        let (s, f) = with_mastery(snapshot(), 0.5);
        let offer: Vec<u16> = (0..12).collect();
        let ally = vec![s.meta.names.get("a").unwrap()];
        for cand in [f, s.meta.names.get("c").unwrap(), s.meta.names.get("l").unwrap()] {
            for ban in [false, true] {
                let before = value(&s, cand, &ally, &[], ban, &offer);
                let at = |lineups: &Lineups, w: f32| {
                    value_with(&s, cand, &Draft { ally: &ally, enemy: &[], offer: &offer, lineups }, ban, w).total
                };
                let strangers: Lineup = Role::ALL.iter().map(|r| (900 + r.index() as u32, Some(*r))).collect();
                let unknown = Lineups { ally: strangers.clone(), enemy: strangers };
                assert_eq!(at(&unknown, 0.5), before, "players the model does not know");
                assert_eq!(at(&Lineups::default(), 0.5), before, "no line-ups");
                let fielded = Lineups { ally: lineup_with(501, Role::Top), enemy: lineup_with(501, Role::Top) };
                for lane in Role::ALL {
                    let all = Lineups { ally: lineup_with(501, lane), enemy: lineup_with(501, lane) };
                    assert_eq!(at(&all, 0.0), before, "player_mastery = 0");
                }
                assert_eq!(at(&fielded, 0.0), before);
            }
        }
    }

    #[test]
    fn a_nudge_not_an_override() {
        // however lopsided the model's mastery, the players' part is capped: at the default
        // weight it moves the score by at most half the strength (tanh is 1-Lipschitz)
        let (s, f) = with_mastery(snapshot(), 2.0);
        let offer: Vec<u16> = (0..12).collect();
        let none = Lineups::default();
        let lane = value_with(&s, f, &Draft { ally: &[], enemy: &[], offer: &offer, lineups: &none }, false, 0.5).role.unwrap();
        let ours = Lineups { ally: lineup_with(501, lane), enemy: Vec::new() };
        let cfg = config::Config::default();
        let v = value_with(&s, f, &Draft { ally: &[], enemy: &[], offer: &offer, lineups: &ours }, false, cfg.player_mastery);
        assert!((v.mastery - PLAYER_CAP * cfg.player_mastery).abs() < 1e-6, "{v:?}");
        let before = amount(value(&s, f, &[], &[], false, &offer), cfg.edge_scale, cfg.pick_strength);
        let after = amount(v.total, cfg.edge_scale, cfg.pick_strength);
        assert!(after > before && after - before <= 0.5 * cfg.pick_strength + 1e-6, "{before} -> {after}");
        assert!(after < cfg.pick_strength);
    }

    /// One AI decision through the hook as the game makes it, with what `diag.log` gets.
    #[test]
    fn the_hook_uses_the_line_ups_and_logs_them() {
        use mod_api_stable::{ChampionBriefV1, DraftCtxV1, DraftPhaseV1, StrV1};
        let _serial = crate::tests::serial();
        let dir = std::env::temp_dir().join(format!("patch_meta_ai_draft_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        std::env::set_var(crate::paths::DIR_ENV, &dir);
        crate::reset_for_tests();

        let (mut s, f) = with_mastery(snapshot(), 0.4);
        s.athlete_names.insert(501, "Faker".into());
        s.team_names.insert(12, "T1".into());
        let names: Vec<String> = NAMES.iter().map(|n| n.to_string()).collect();
        let briefs: Vec<ChampionBriefV1> =
            names.iter().map(|n| ChampionBriefV1 { name: StrV1::from_str(n), ..Default::default() }).collect();
        let available: Vec<usize> = (0..names.len()).collect();
        let none = Lineups::default();
        let offer: Vec<u16> = (0..12).collect();
        let lane = value_with(&s, f, &Draft { ally: &[], enemy: &[], offer: &offer, lineups: &none }, false, 0.5).role.unwrap();
        let ours: Vec<usize> = lineup_with(501, lane).iter().map(|(a, _)| *a as usize).collect();
        let theirs = [31usize, 32, 33, 34, 35];
        shared::publish(s);

        let ctx_with = |athletes: &[usize], enemy_athletes: &[usize]| {
            // SAFETY: a plain C struct; every pointer set below outlives the calls
            let mut raw: DraftCtxV1 = unsafe { std::mem::zeroed() };
            raw.size = std::mem::size_of::<DraftCtxV1>();
            raw.phase = DraftPhaseV1::Pick.code();
            raw.available_ptr = available.as_ptr();
            raw.available_len = available.len();
            raw.champion_briefs_ptr = briefs.as_ptr();
            raw.champion_briefs_len = briefs.len();
            raw.ally_athletes_ptr = athletes.as_ptr();
            raw.ally_athletes_len = athletes.len();
            raw.enemy_athletes_ptr = enemy_athletes.as_ptr();
            raw.enemy_athletes_len = enemy_athletes.len();
            raw.ally_team = 12;
            raw.enemy_team = 60;
            raw
        };
        let ask = |raw: &DraftCtxV1, cand: usize| -> f32 {
            // SAFETY: `raw` lives for the call
            let ctx = unsafe { StableDraftContext::from_raw(raw) }.unwrap();
            match MetaDraftHook.score_pick(&ctx, cand, 0.0) {
                StableDraftDecision::Add(v) => v,
                _ => 0.0,
            }
        };
        set_live(true);
        let fielded = ctx_with(&ours, &theirs);
        let scores: Vec<f32> = available.iter().map(|c| ask(&fielded, *c)).collect();
        let bare = ctx_with(&[], &[]);
        let before: Vec<f32> = available.iter().map(|c| ask(&bare, *c)).collect();
        assert!(scores[f as usize] > before[f as usize], "{} vs {}", scores[f as usize], before[f as usize]);
        for c in available.iter().filter(|c| **c != f as usize) {
            // the others' players are not known to the model: nothing changes for them
            assert_eq!(scores[*c], before[*c], "{}", names[*c]);
        }
        set_live(false);

        let log = std::fs::read_to_string(dir.join(diag::LOG_FILE)).unwrap();
        assert!(log.contains("[draft] line-ups on screen: T1 (#12) ["), "{log}");
        assert!(log.contains(&format!("{} #501 Faker", lane.name())), "{log}");
        assert!(log.contains("[draft] T1 (#12) pick 1 vs team #60, on screen (12 asked): "), "{log}");
        // 0.4 x player_mastery 0.5 = +0.2 log-odds = +5.0 win-rate points
        assert!(log.contains(&format!(" (players +5.0: {} #501 Faker) -> +", lane.name())), "{log}");
        // the same decision again (the game asks once more): not written twice
        let again: Vec<f32> = available.iter().map(|c| ask(&fielded, *c)).collect();
        assert_eq!(again, scores);
        let log2 = std::fs::read_to_string(dir.join(diag::LOG_FILE)).unwrap();
        assert_eq!(log2.matches("line-ups on screen").count(), 1);
        crate::reset_for_tests();
        shared::clear();
    }

    #[test]
    fn the_coach_follows_the_panel_else_the_model() {
        let _serial = crate::tests::serial();
        crate::reset_for_tests();
        let s = snapshot();
        let id = |n: &str| s.meta.names.get(n).unwrap();
        let available: Vec<(usize, u16)> = (0..12u16).map(|c| (c as usize + 100, c)).collect();
        let none = Lineups::default();
        // no plan: the model's best pick, c
        shared::set_coach_plan(None);
        assert_eq!(coach_choice(&s, &[], &[], 6, false, &available, &none, 0.5), Some((id("c"), CoachSource::Model)));
        // the panel's ranking for this very draft (picks in any order)
        let plan = shared::CoachPlan {
            ally: vec![],
            enemy: vec![],
            bans: 6,
            ban: false,
            ranked: vec![id("e"), id("f")],
            held: vec![id("c")],
        };
        shared::set_coach_plan(Some(plan.clone()));
        assert_eq!(coach_choice(&s, &[], &[], 6, false, &available, &none, 0.5), Some((id("e"), CoachSource::Panel)));
        // its first choice taken meanwhile: the next one
        let fewer: Vec<(usize, u16)> = available.iter().copied().filter(|(_, c)| *c != id("e")).collect();
        assert_eq!(coach_choice(&s, &[], &[], 6, false, &fewer, &none, 0.5), Some((id("f"), CoachSource::Panel)));
        // a plan for another moment of the draft: the model, never a card a position lock holds
        let (choice, source) = coach_choice(&s, &[], &[], 7, false, &available, &none, 0.5).unwrap();
        assert_eq!(source, CoachSource::Model);
        assert_ne!(choice, id("c"), "held by the position lock");
        // bans: the panel's when it was drawn for a ban, the model's else
        assert_eq!(coach_choice(&s, &[], &[], 6, true, &available, &none, 0.5).map(|c| c.1), Some(CoachSource::Model));
        shared::set_coach_plan(Some(shared::CoachPlan { ban: true, ranked: vec![id("k")], ..plan }));
        assert_eq!(coach_choice(&s, &[], &[], 6, true, &available, &none, 0.5), Some((id("k"), CoachSource::Panel)));
        shared::set_coach_plan(None);
    }

    /// The coach override through the hook: the player's team only, and its choice is noted.
    #[test]
    fn the_coach_decides_for_the_player_only() {
        use mod_api_stable::{ChampionBriefV1, DraftCtxV1, DraftPhaseV1, StrV1};
        let _serial = crate::tests::serial();
        crate::reset_for_tests();
        let names: Vec<String> = NAMES.iter().map(|n| n.to_string()).collect();
        let briefs: Vec<ChampionBriefV1> =
            names.iter().map(|n| ChampionBriefV1 { name: StrV1::from_str(n), ..Default::default() }).collect();
        let available: Vec<usize> = (0..names.len()).collect();
        let ctx_for = |team: usize| {
            // SAFETY: a plain C struct; every pointer set below outlives the calls
            let mut raw: DraftCtxV1 = unsafe { std::mem::zeroed() };
            raw.size = std::mem::size_of::<DraftCtxV1>();
            raw.phase = DraftPhaseV1::Pick.code();
            raw.available_ptr = available.as_ptr();
            raw.available_len = available.len();
            raw.champion_briefs_ptr = briefs.as_ptr();
            raw.champion_briefs_len = briefs.len();
            raw.ally_team = team;
            raw.enemy_team = 60;
            raw
        };
        let decide = |raw: &DraftCtxV1| {
            // SAFETY: `raw` lives for the call
            let ctx = unsafe { StableDraftContext::from_raw(raw) }.unwrap();
            CoachHook.decide_pick(&ctx)
        };
        let ours = ctx_for(19);
        assert_eq!(decide(&ours), None, "no model yet");
        let s = snapshot();
        let c = s.meta.names.get("c").unwrap();
        shared::publish(s);
        assert_eq!(decide(&ours), None, "no save open: no player team");
        shared::set_player_team(Some(19));
        shared::take_coach_choices();
        assert_eq!(decide(&ours), Some(c as usize));
        assert_eq!(decide(&ctx_for(12)), None, "another team");
        assert_eq!(shared::take_coach_choices(), vec![(false, c)]);
        assert_eq!(CoachHook.priority(), i32::MAX);
        shared::set_player_team(None);
        shared::clear();
        crate::reset_for_tests();
    }

    /// ai_draft: every AI team decides by the model; the player's team is left to the coach.
    #[test]
    fn ai_draft_decides_for_the_ai_teams() {
        use mod_api_stable::{ChampionBriefV1, DraftCtxV1, DraftPhaseV1, StrV1};
        let _serial = crate::tests::serial();
        let dir = std::env::temp_dir().join(format!("patch_meta_ai_ai_draft_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        std::env::set_var(crate::paths::DIR_ENV, &dir);
        crate::reset_for_tests();
        let names: Vec<String> = NAMES.iter().map(|n| n.to_string()).collect();
        let briefs: Vec<ChampionBriefV1> =
            names.iter().map(|n| ChampionBriefV1 { name: StrV1::from_str(n), ..Default::default() }).collect();
        let available: Vec<usize> = (0..names.len()).collect();
        let ctx_for = |team: usize, phase: DraftPhaseV1| {
            // SAFETY: a plain C struct; every pointer set below outlives the calls
            let mut raw: DraftCtxV1 = unsafe { std::mem::zeroed() };
            raw.size = std::mem::size_of::<DraftCtxV1>();
            raw.phase = phase.code();
            raw.available_ptr = available.as_ptr();
            raw.available_len = available.len();
            raw.champion_briefs_ptr = briefs.as_ptr();
            raw.champion_briefs_len = briefs.len();
            raw.ally_team = team;
            raw.enemy_team = 19;
            raw
        };
        let decide = |raw: &DraftCtxV1, ban: bool| {
            // SAFETY: `raw` lives for the call
            let ctx = unsafe { StableDraftContext::from_raw(raw) }.unwrap();
            if ban { CoachHook.decide_ban(&ctx) } else { CoachHook.decide_pick(&ctx) }
        };
        let s = snapshot();
        let c = s.meta.names.get("c").unwrap();
        shared::publish(s);
        shared::set_player_team(Some(19));
        let ai_pick = ctx_for(12, DraftPhaseV1::Pick);
        assert_eq!(decide(&ai_pick, false), None, "ai_draft is off by default");
        config::set_for_tests(config::Config { ai_draft: true, ..config::Config::default() });
        set_live(true);
        shared::take_ai_choices();
        assert_eq!(decide(&ai_pick, false), Some(c as usize), "the strongest");
        assert!(decide(&ctx_for(12, DraftPhaseV1::Ban), true).is_some());
        assert_eq!(shared::take_ai_choices().first(), Some(&(false, c)), "noted for the review");
        // the player's own team: the coach override, never ai_draft
        let ours = ctx_for(19, DraftPhaseV1::Pick);
        // SAFETY: `ours` lives for the call
        assert_eq!(ai_decide(&unsafe { StableDraftContext::from_raw(&ours) }.unwrap(), false), None);
        set_live(false);
        let log = std::fs::read_to_string(dir.join(diag::LOG_FILE)).unwrap();
        assert!(log.contains("[ai] team #12 pick 1 vs team #19, on screen: c"), "{log}");
        shared::set_player_team(None);
        shared::clear();
        crate::reset_for_tests();
    }

    /// With Smart Position Lock holding AI picks, ai_draft only takes what the lock would let the
    /// team seat - here the strongest, c, plays only Mid and the team's a already does.
    #[test]
    fn ai_draft_keeps_to_the_locks_lanes() {
        let _serial = crate::tests::serial();
        crate::reset_for_tests();
        let s = snapshot();
        let id = |n: &str| s.meta.names.get(n).unwrap();
        let available: Vec<(usize, u16)> = (0..12u16).filter(|c| *c != id("a")).map(|c| (c as usize, c)).collect();
        let none = Lineups::default();
        assert_eq!(ai_choice(&s, &[id("a")], &[], 6, false, &available, &none, [None, None]), Some(id("c")), "no lock: the strongest");
        let table = crate::lock::parse("[lock]
ai=on
[positions]
a=Mid
c=Mid
", "", "");
        crate::lock::set(Some(table.clone()));
        let pick = ai_choice(&s, &[id("a")], &[], 6, false, &available, &none, [None, None]).unwrap();
        assert_ne!(pick, id("c"), "c cannot be seated next to a");
        // bans are not held
        assert!(ai_choice(&s, &[id("a")], &[], 6, true, &available, &none, [None, None]).is_some());
        // the lock's AI switch off: not held
        crate::lock::set(Some(crate::lock::Table { ai: false, ..table }));
        assert_eq!(ai_choice(&s, &[id("a")], &[], 6, false, &available, &none, [None, None]), Some(id("c")));
        crate::lock::set(None);
    }

    /// ai_variety: one draw per draft, kept through it; a new draft draws again; a champion the
    /// model is sure of barely moves; and over many drafts the AI opens with more champions.
    #[test]
    fn variety_draws_once_per_draft_and_spreads_the_picks() {
        let _serial = crate::tests::serial();
        crate::reset_for_tests();
        let base = Arc::new(snapshot());
        let teams = (Some(12), Some(19));
        assert!(Arc::ptr_eq(&variety::sampled(&base, teams, 0, 0.0), &base), "0: the model itself");
        let first = variety::sampled(&base, teams, 0, 1.0);
        let later = variety::sampled(&base, teams, 7, 1.0);
        assert!(Arc::ptr_eq(&first, &later), "the same draft keeps its draw");
        let next = variety::sampled(&base, teams, 0, 1.0);
        assert!(!Arc::ptr_eq(&first, &next), "a new draft draws again");
        // every champion moves by its own uncertainty
        for (b, n) in base.meta.champions.iter().zip(&next.meta.champions) {
            assert!((n.strength - b.strength).abs() <= 5.0 * b.sd + 1e-6, "{}", b.name);
        }
        // the AI's first pick over forty drafts: always the same without variety, more with it
        let available: Vec<(usize, u16)> = (0..12u16).map(|c| (c as usize, c)).collect();
        let none = Lineups::default();
        let first_picks = |weight: f32| {
            let mut seen = std::collections::HashSet::new();
            for _ in 0..40 {
                let s = variety::sampled(&base, (Some(1), Some(2)), 0, weight);
                let _ = variety::sampled(&base, (Some(1), Some(2)), 99, weight);
                seen.insert(ai_choice(&s, &[], &[], 6, false, &available, &none, [None, None]).unwrap());
            }
            seen.len()
        };
        assert_eq!(first_picks(0.0), 1);
        assert!(first_picks(2.0) > 1, "variety opens with other champions too");
        variety::reset();
    }

    /// The AI scouts: a champion the enemy plays in every match is likelier to be banned.
    #[test]
    fn ai_draft_bans_what_the_enemy_plays() {
        let _serial = crate::tests::serial();
        crate::reset_for_tests();
        let s = snapshot();
        let available: Vec<(usize, u16)> = (0..12u16).map(|c| (c as usize, c)).collect();
        let none = Lineups::default();
        let offer: Vec<u16> = (0..12).collect();
        let board = |habits: Option<&HashMap<u16, f32>>| {
            let them = advisor::Team { habits, ..advisor::Team::new(&[], &[]) };
            let plan = advisor::plan_turn(
                &s.meta,
                &advisor::TurnInput {
                    us: advisor::Team::new(&[], &[]),
                    them,
                    offer: &offer,
                    pickable: &offer,
                    banning: true,
                    blue: None,
                    bans_made: 0,
                    lookahead: false,
                    conserve: 0.0,
                    also: &[],
                },
                &|_| None,
            );
            plan.board.iter().map(|b| (b.champ, b.total)).collect::<HashMap<u16, f32>>()
        };
        let f = s.meta.names.get("f").unwrap();
        let fond: HashMap<u16, f32> = [(f, 1.0)].into_iter().collect();
        assert!(board(Some(&fond))[&f] > board(None)[&f], "their habit makes it a better ban");
        // through ai_choice: the enemy's habits are the second
        assert!(ai_choice(&s, &[], &[], 0, true, &available, &none, [None, Some(&fond)]).is_some());
    }

    /// On a real save: AI-vs-AI drafts, blue with variety and red without - how many champions
    /// blue picks, the ten most picked's share, and blue's win chance at the end (the cost).
    /// `PMA_HISTORY=<history file> cargo test --release --lib variety_on_a_save -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn variety_on_a_save() {
        let Ok(path) = std::env::var("PMA_HISTORY") else { return };
        let _serial = crate::tests::serial();
        crate::reset_for_tests();
        let text = std::fs::read_to_string(&path).unwrap();
        let mut names = crate::history::Names::default();
        let games: Vec<_> = crate::cache::decode(&text, &mut names).into_iter().map(|(_, g)| g).collect();
        let mut champions: Vec<String> = crate::knowledge::get().positions.keys().cloned().collect();
        champions.sort();
        let current = games.iter().map(|g| g.version.clone()).max().unwrap_or_default();
        let meta = build(&Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: &current, warm: None, stats: None }, &Settings::default());
        let pool: Vec<u16> = champions.iter().filter_map(|c| meta.names.get(c)).collect();
        let base = Arc::new(Snapshot::new(meta, HashMap::new()));
        let none = Lineups::default();
        for weight in [0.0f32, 0.5, 1.0, 2.0, 4.0] {
            let mut picked: HashMap<u16, u32> = HashMap::new();
            let mut cost = 0.0f32;
            let started = std::time::Instant::now();
            for d in 0..60usize {
                let (mut picks, mut bans): ([Vec<u16>; 2], usize) = ([Vec::new(), Vec::new()], 0);
                let mut taken: Vec<u16> = Vec::new();
                for step in advisor::ORDER_3_2 {
                    let me = if step.blue { 0 } else { 1 };
                    let teams = (Some(2 * d + me), Some(2 * d + 1 - me));
                    let progress = bans + picks[0].len() + picks[1].len();
                    // blue drafts with variety, red without: what variety costs
                    let available: Vec<(usize, u16)> = pool.iter().filter(|c| !taken.contains(c)).map(|c| (*c as usize, *c)).collect();
                    let c = if me == 0 && weight > 0.0 {
                        let s = variety::sampled(&base, teams, progress, 1.0);
                        varied_choice(&base, &s, weight, &picks[me], &picks[1 - me], bans, step.ban, &available, &none, [None, None])
                    } else {
                        ai_choice(&base, &picks[me], &picks[1 - me], bans, step.ban, &available, &none, [None, None])
                    }
                    .unwrap();
                    taken.push(c);
                    if step.ban {
                        bans += 1;
                    } else {
                        picks[me].push(c);
                        if me == 0 {
                            *picked.entry(c).or_default() += 1;
                        }
                    }
                }
                cost += advisor::win_probability(&base.meta, &picks[0], &picks[1]);
            }
            let total: u32 = picked.values().sum();
            let mut counts: Vec<u32> = picked.values().copied().collect();
            counts.sort_unstable_by(|a, b| b.cmp(a));
            let top10: u32 = counts.iter().take(10).sum();
            eprintln!(
                "ai_variety {weight}: 60 drafts, {} champions picked of {}, top 10 = {:.0}% of picks, its win chance vs no variety {:.1}% ({:?})",
                picked.len(),
                pool.len(),
                top10 as f32 / total as f32 * 100.0,
                cost / 60.0 * 100.0,
                started.elapsed()
            );
            variety::reset();
        }
    }
}
