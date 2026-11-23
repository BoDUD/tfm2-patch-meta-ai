//! 你的比赛前后的新闻（2.14.0）：赛前前瞻和赛后复盘。用户 2026-11-23 说现在玩得有点无聊，在几个方向里选了这一个。
//!
//! 赛程在游戏的 MatchNormal 记录里（一条一个系列赛：日期、两支队、几胜制、状态 Wait / End、每局的 MatchReplay 记录）。
//! 你的比赛当天、开打之前发一条前瞻：积分榜、双方近 5 场、交手记录、对方风格和招牌英雄、对方可能先禁谁、对方关键
//! 选手、只看选手的赛前胜率。系列赛打完发一条复盘：每局的方、时长、胜负、击杀、经济、小龙大龙、MVP，最后是比分和
//! 你方这个系列赛最好的选手。这里只放读记录和写文字的纯函数，什么时候发由 `client` 决定。

use serde_json::Value;

/// 一个系列赛的状态
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Wait,
    /// 打完了：赢的队、两边各赢几局（team1, team2）
    End { winner: u32, score: [u32; 2] },
    /// 别的（正在打，或者认不出来）
    Other,
}

/// 一个系列赛（MatchNormal 记录）
#[derive(Clone, Debug, PartialEq)]
pub struct Fixture {
    pub id: usize,
    pub date: (i32, u32, u32),
    /// 两支队（`WinnerOf` 这种还不知道是谁的是 None）
    pub teams: [Option<u32>; 2],
    pub need_win: u32,
    pub state: State,
    pub replays: Vec<usize>,
    pub practice: bool,
}

impl Fixture {
    pub fn has(&self, team: u32) -> bool {
        self.teams.contains(&Some(team))
    }

    /// 另一支队
    pub fn opponent(&self, team: u32) -> Option<u32> {
        match self.teams {
            [Some(a), b] if a == team => b,
            [a, Some(b)] if b == team => a,
            _ => None,
        }
    }

    /// 几局几胜（need_win 2 = BO3）
    pub fn best_of(&self) -> u32 {
        (self.need_win * 2).saturating_sub(1).max(1)
    }
}

fn team_of(v: &Value) -> Option<u32> {
    v.get("Normal").and_then(Value::as_u64).and_then(|t| u32::try_from(t).ok())
}

/// "2028-10-22T19:00:00" 的日期
pub fn date_of(text: &str) -> Option<(i32, u32, u32)> {
    let day = text.get(..10)?;
    let mut parts = day.split('-');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

pub fn parse_fixture(id: usize, json: &str) -> Option<Fixture> {
    let v: Value = serde_json::from_str(json).ok()?;
    let date = date_of(v.get("date")?.as_str()?)?;
    let state = match v.get("running_state") {
        Some(Value::String(s)) if s == "Wait" => State::Wait,
        Some(Value::Object(m)) => match m.get("End") {
            Some(end) => {
                let num = |k: &str| end.get(k).and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok());
                State::End { winner: num("winner")?, score: [num("team1_score").unwrap_or(0), num("team2_score").unwrap_or(0)] }
            }
            None => State::Other,
        },
        _ => State::Other,
    };
    let replays = v
        .get("replays")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_u64).map(|r| r as usize).collect())
        .unwrap_or_default();
    Some(Fixture {
        id,
        date,
        teams: [v.get("team1").and_then(team_of), v.get("team2").and_then(team_of)],
        need_win: v.get("need_win").and_then(Value::as_u64).map_or(1, |n| n as u32),
        state,
        replays,
        practice: v.get("is_practice").and_then(Value::as_bool).unwrap_or(false),
    })
}

/// 一支队最近打完的系列赛（`before` 那天之前，最多 `n` 场，旧的在前）：赢没赢
pub fn form(fixtures: &[Fixture], team: u32, before: (i32, u32, u32), n: usize) -> Vec<bool> {
    let mut done: Vec<&Fixture> =
        fixtures.iter().filter(|f| f.has(team) && !f.practice && f.date < before && matches!(f.state, State::End { .. })).collect();
    done.sort_by_key(|f| (f.date, f.id));
    let from = done.len().saturating_sub(n);
    done[from..]
        .iter()
        .filter_map(|f| match f.state {
            State::End { winner, .. } => Some(winner == team),
            _ => None,
        })
        .collect()
}

/// 积分榜的一行：(队, 胜, 负, 小局净胜)
pub type Row = (u32, u32, u32, i64);

/// 联赛积分榜（LeagueCompetition 记录的 standings），排好名次
pub fn standings(json: &str) -> Vec<Row> {
    let Ok(v) = serde_json::from_str::<Value>(json) else { return Vec::new() };
    let Some(map) = v.get("standings").and_then(Value::as_object) else { return Vec::new() };
    let num = |e: &Value, k: &str| e.get(k).and_then(Value::as_u64).unwrap_or(0);
    let mut out: Vec<Row> = map
        .iter()
        .filter_map(|(team, e)| {
            let team = team.parse::<u32>().ok()?;
            Some((team, num(e, "win") as u32, num(e, "lose") as u32, num(e, "set_win") as i64 - num(e, "set_lose") as i64))
        })
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(b.3.cmp(&a.3)).then(a.2.cmp(&b.2)).then(a.0.cmp(&b.0)));
    out
}

/// 名次（从 1 数）、一共几队、胜、负
pub fn rank_of(table: &[Row], team: u32) -> Option<(usize, usize, u32, u32)> {
    let at = table.iter().position(|t| t.0 == team)?;
    Some((at + 1, table.len(), table[at].1, table[at].2))
}

/// 一局里的一个选手
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayerLine {
    pub athlete: Option<u32>,
    pub champion: String,
    pub position: String,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    /// 游戏里的评分（记录里是 0-100，显示时除以 10）
    pub rating: f32,
    pub mvp: bool,
}

/// 一局里的一方
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameSide {
    pub team: Option<u32>,
    pub won: bool,
    pub kills: u32,
    pub gold: u64,
    pub drakes: u32,
    pub barons: u32,
    pub first_drake: bool,
    pub first_baron: bool,
    pub players: Vec<PlayerLine>,
}

/// 一局（MatchReplay 记录）：打了多少 tick，蓝方、红方
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameRecord {
    pub ticks: u64,
    pub sides: [GameSide; 2],
}

pub fn parse_game(json: &str) -> Option<GameRecord> {
    let v: Value = serde_json::from_str(json).ok()?;
    let blue_win = v.get("blue_team_win")?.as_bool()?;
    let side = |prefix: &str, won: bool| -> GameSide {
        let perf = v.get(format!("{prefix}_performance"));
        let num = |k: &str| perf.and_then(|p| p.get(k)).and_then(Value::as_u64).unwrap_or(0);
        let flag = |k: &str| perf.and_then(|p| p.get(k)).and_then(Value::as_bool).unwrap_or(false);
        let players = v
            .get(format!("{prefix}_team"))
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .map(|p| {
                        let st = p.get("statistics");
                        let n = |k: &str| st.and_then(|s| s.get(k)).and_then(Value::as_u64).unwrap_or(0);
                        PlayerLine {
                            athlete: p.get("athlete_id").and_then(Value::as_u64).and_then(|a| u32::try_from(a).ok()),
                            champion: p.get("champion").and_then(Value::as_str).unwrap_or_default().to_string(),
                            position: p.get("position").and_then(Value::as_str).unwrap_or_default().to_string(),
                            kills: n("kills") as u32,
                            deaths: n("deaths") as u32,
                            assists: n("assists") as u32,
                            rating: n("rating") as f32,
                            mvp: n("mvp") > 0,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        GameSide {
            team: v.get(format!("{prefix}_team_id")).and_then(Value::as_u64).and_then(|t| u32::try_from(t).ok()),
            won,
            kills: num("total_kills") as u32,
            gold: num("total_gold"),
            drakes: num("serpen_secured") as u32,
            barons: num("epic_secured") as u32,
            first_drake: flag("first_serpen"),
            first_baron: flag("first_epic"),
            players,
        }
    };
    Some(GameRecord { ticks: v.get("game_tick").and_then(Value::as_u64).unwrap_or(0), sides: [side("blue", blue_win), side("red", !blue_win)] })
}

/// 前瞻里的一支队
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TeamInfo {
    pub name: String,
    /// 名次、一共几队、胜、负
    pub rank: Option<(usize, usize, u32, u32)>,
    /// 近几场（旧的在前）
    pub form: Vec<bool>,
}

/// 赛前前瞻要写的东西
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Preview {
    pub us: TeamInfo,
    pub them: TeamInfo,
    pub best_of: u32,
    /// 这个存档里两队打过的局：我们赢几局、对方赢几局
    pub head_to_head: (u32, u32),
    pub them_style: Option<String>,
    pub them_signature: Vec<String>,
    pub likely_bans: Vec<String>,
    /// 对方关键选手：名字、位置
    pub key_players: Vec<(String, String)>,
    /// 只看双方选手，我们的赛前胜率
    pub strength: Option<f32>,
}

/// 位置的中文名
pub fn lane_name(position: &str, chinese: bool) -> String {
    let zh = match position.to_ascii_lowercase().as_str() {
        "top" => "上单",
        "jungle" => "打野",
        "mid" => "中单",
        "bottom" => "下路",
        "support" => "辅助",
        _ => return position.to_string(),
    };
    if chinese {
        zh.to_string()
    } else {
        position.to_string()
    }
}

fn form_text(form: &[bool], chinese: bool) -> String {
    if form.is_empty() {
        return if chinese { "还没打过".to_string() } else { "no matches yet".to_string() };
    }
    let marks: Vec<&str> = form
        .iter()
        .map(|w| match (chinese, *w) {
            (true, true) => "胜",
            (true, false) => "负",
            (false, true) => "W",
            (false, false) => "L",
        })
        .collect();
    marks.join(if chinese { "" } else { " " })
}

pub fn preview_news(p: &Preview, chinese: bool) -> (String, String) {
    let title = match (chinese, p.best_of > 1) {
        (true, true) => format!("赛前前瞻：{} vs {}（BO{}）", p.us.name, p.them.name, p.best_of),
        (true, false) => format!("赛前前瞻：{} vs {}", p.us.name, p.them.name),
        (false, true) => format!("Match preview: {} vs {} (Bo{})", p.us.name, p.them.name, p.best_of),
        (false, false) => format!("Match preview: {} vs {}", p.us.name, p.them.name),
    };
    let mut lines = Vec::new();
    if let (Some(a), Some(b)) = (p.us.rank, p.them.rank) {
        lines.push(if chinese {
            format!("积分榜：{} 第 {}（{} 胜 {} 负），{} 第 {}（{} 胜 {} 负）", p.us.name, a.0, a.2, a.3, p.them.name, b.0, b.2, b.3)
        } else {
            format!("Standings: {} #{} ({}-{}), {} #{} ({}-{})", p.us.name, a.0, a.2, a.3, p.them.name, b.0, b.2, b.3)
        });
    }
    lines.push(if chinese {
        format!("近况：{} {}；{} {}", p.us.name, form_text(&p.us.form, true), p.them.name, form_text(&p.them.form, true))
    } else {
        format!("Form: {} {}; {} {}", p.us.name, form_text(&p.us.form, false), p.them.name, form_text(&p.them.form, false))
    });
    let (a, b) = p.head_to_head;
    lines.push(match (a + b, chinese) {
        (0, true) => "交手：这个存档里两队还没交过手".to_string(),
        (0, false) => "Head to head: the two have not met in this save".to_string(),
        (n, true) => format!("交手：这个存档里两队打过 {n} 局，{} {a} 胜 {b} 负", p.us.name),
        (n, false) => format!("Head to head: {n} games in this save, {} won {a}, lost {b}", p.us.name),
    });
    let style = p.them_style.clone().unwrap_or_default();
    let sig = p.them_signature.join(if chinese { "、" } else { ", " });
    match (style.is_empty(), sig.is_empty(), chinese) {
        (true, true, _) => {}
        (false, false, true) => lines.push(format!("对方风格：{style}；招牌英雄：{sig}")),
        (false, true, true) => lines.push(format!("对方风格：{style}")),
        (true, false, true) => lines.push(format!("对方招牌英雄：{sig}")),
        (false, false, false) => lines.push(format!("Their style: {style}; signature champions: {sig}")),
        (false, true, false) => lines.push(format!("Their style: {style}")),
        (true, false, false) => lines.push(format!("Their signature champions: {sig}")),
    }
    if !p.likely_bans.is_empty() {
        lines.push(if chinese {
            format!("对方可能先禁：{}", p.likely_bans.join("、"))
        } else {
            format!("They will likely ban first: {}", p.likely_bans.join(", "))
        });
    }
    if !p.key_players.is_empty() {
        let list: Vec<String> = p
            .key_players
            .iter()
            .map(|(name, lane)| if chinese { format!("{name}（{}）", lane_name(lane, true)) } else { format!("{name} ({lane})") })
            .collect();
        lines.push(if chinese { format!("对方关键选手：{}", list.join("、")) } else { format!("Their key players: {}", list.join(", ")) });
    }
    if let Some(s) = p.strength {
        lines.push(if chinese {
            format!("只看双方选手，{} 赛前胜率约 {:.0}%（还没算 BP）", p.us.name, s * 100.0)
        } else {
            format!("On the players alone {} win about {:.0}% (before the draft)", p.us.name, s * 100.0)
        });
    }
    (title, lines.join("\n"))
}

/// 一局的时长：分、秒（60 tick 一秒）
pub fn minutes(ticks: u64) -> (u64, u64) {
    let secs = ticks / 60;
    (secs / 60, secs % 60)
}

/// 赛后复盘：`us` 是你的队，`names` 是（你方, 对方）的队名，`score` 是两边赢的局数，`games` 按顺序；`athlete` 给选手
/// 名字，`champion` 给英雄名字
pub fn review_news(
    us: u32,
    names: (&str, &str),
    score: (u32, u32),
    games: &[GameRecord],
    chinese: bool,
    athlete: &dyn Fn(u32) -> String,
    champion: &dyn Fn(&str) -> String,
) -> (String, String) {
    let (our_name, their_name) = names;
    let title = if chinese {
        format!("赛后复盘：{our_name} {}:{} {their_name}", score.0, score.1)
    } else {
        format!("Match review: {our_name} {}-{} {their_name}", score.0, score.1)
    };
    let mut lines = Vec::new();
    // 你方每个选手：评分加起来、打了几局
    let mut ours: Vec<(u32, f32, u32)> = Vec::new();
    for (n, g) in games.iter().enumerate() {
        let Some(me) = g.sides.iter().position(|s| s.team == Some(us)) else { continue };
        let (a, b) = (&g.sides[me], &g.sides[1 - me]);
        let (mm, ss) = minutes(g.ticks);
        let everyone = || g.sides.iter().flat_map(|s| s.players.iter());
        let mvp = everyone()
            .find(|p| p.mvp)
            .or_else(|| everyone().max_by(|x, y| x.rating.partial_cmp(&y.rating).unwrap_or(std::cmp::Ordering::Equal)));
        let mvp_text = mvp
            .map(|p| {
                let who = p.athlete.map(athlete).unwrap_or_default();
                if chinese {
                    format!("。MVP {who}（{} {}/{}/{}）", champion(&p.champion), p.kills, p.deaths, p.assists)
                } else {
                    format!(". MVP {who} ({} {}/{}/{})", champion(&p.champion), p.kills, p.deaths, p.assists)
                }
            })
            .unwrap_or_default();
        let firsts = match (a.first_drake, a.first_baron, chinese) {
            (true, true, true) => "，先拿第一条小龙和大龙",
            (true, false, true) => "，先拿第一条小龙",
            (false, true, true) => "，先拿大龙",
            (true, true, false) => ", took the first drake and Baron",
            (true, false, false) => ", took the first drake",
            (false, true, false) => ", took the first Baron",
            _ => "",
        };
        let (k, gold, d, bar) = (
            (a.kills, b.kills),
            (a.gold as f64 / 1000.0, b.gold as f64 / 1000.0),
            (a.drakes, b.drakes),
            (a.barons, b.barons),
        );
        lines.push(if chinese {
            format!(
                "第 {} 局（{}方，{mm} 分 {ss:02} 秒）{}：击杀 {}:{}，经济 {:.1}k:{:.1}k，小龙 {}:{}，大龙 {}:{}{firsts}{mvp_text}",
                n + 1,
                if me == 0 { "蓝" } else { "红" },
                if a.won { "胜" } else { "负" },
                k.0,
                k.1,
                gold.0,
                gold.1,
                d.0,
                d.1,
                bar.0,
                bar.1
            )
        } else {
            format!(
                "Game {} ({} side, {mm}:{ss:02}) {}: kills {}-{}, gold {:.1}k-{:.1}k, drakes {}-{}, Baron {}-{}{firsts}{mvp_text}",
                n + 1,
                if me == 0 { "blue" } else { "red" },
                if a.won { "won" } else { "lost" },
                k.0,
                k.1,
                gold.0,
                gold.1,
                d.0,
                d.1,
                bar.0,
                bar.1
            )
        });
        for p in &a.players {
            let Some(id) = p.athlete else { continue };
            match ours.iter_mut().find(|o| o.0 == id) {
                Some(o) => {
                    o.1 += p.rating;
                    o.2 += 1;
                }
                None => ours.push((id, p.rating, 1)),
            }
        }
    }
    let avg = |o: &(u32, f32, u32)| o.1 / o.2 as f32;
    if let Some(best) = ours.iter().max_by(|x, y| avg(x).partial_cmp(&avg(y)).unwrap_or(std::cmp::Ordering::Equal)) {
        let rating = avg(best) / 10.0;
        lines.push(if chinese {
            format!("本系列赛你方最佳：{}（{} 局平均评分 {rating:.1}）", athlete(best.0), best.2)
        } else {
            format!("Your best player of the series: {} ({} games, average rating {rating:.1})", athlete(best.0), best.2)
        });
    }
    (title, lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAIT: &str = r#"{"date": "2028-10-22T19:00:00", "game_rule": "Game5V5", "id": 7832, "is_practice": false, "need_win": 2,
        "replays": [], "running_state": "Wait", "team1": {"Normal": 78}, "team2": {"Normal": 13}}"#;
    const END: &str = r#"{"date": "2028-10-20T16:30:00", "id": 7700, "is_practice": false, "need_win": 2, "replays": [5, 6, 7],
        "running_state": {"End": {"loser": 13, "team1_score": 1, "team2_score": 2, "winner": 78}},
        "team1": {"Normal": 13}, "team2": {"Normal": 78}}"#;

    #[test]
    fn a_fixture_reads_its_teams_state_and_games() {
        let f = parse_fixture(7832, WAIT).unwrap();
        assert_eq!((f.date, f.teams, f.need_win, f.best_of(), f.state.clone()), ((2028, 10, 22), [Some(78), Some(13)], 2, 3, State::Wait));
        assert_eq!(f.opponent(78), Some(13));
        let e = parse_fixture(7700, END).unwrap();
        assert_eq!(e.state, State::End { winner: 78, score: [1, 2] });
        assert_eq!(e.replays, [5, 6, 7]);
        let later =
            parse_fixture(1, r#"{"date": "2028-10-23T19:00:00", "need_win": 4, "running_state": "Wait", "team1": {"Normal": 51}, "team2": {"WinnerOf": 7831}}"#)
                .unwrap();
        assert_eq!((later.teams, later.best_of()), ([Some(51), None], 7));
        // 近况：只算打完的，旧的在前
        assert_eq!(form(&[f.clone(), e.clone()], 78, (2028, 10, 22), 5), [true]);
        assert_eq!(form(&[f, e], 13, (2028, 10, 22), 5), [false]);
    }

    #[test]
    fn the_standings_rank_by_wins_then_games() {
        let doc = r#"{"standings": {"110": {"win": 15, "lose": 3, "set_win": 31, "set_lose": 10},
            "78": {"win": 15, "lose": 3, "set_win": 33, "set_lose": 9}, "13": {"win": 9, "lose": 9, "set_win": 20, "set_lose": 20}}}"#;
        let table = standings(doc);
        assert_eq!(table.iter().map(|t| t.0).collect::<Vec<_>>(), [78, 110, 13]);
        assert_eq!(rank_of(&table, 13), Some((3, 3, 9, 9)));
        assert!(standings("{}").is_empty());
    }

    fn game(blue: u32, red: u32, blue_win: bool) -> String {
        let player = |a: u32, c: &str, pos: &str, k: u32, d: u32, r: u32, mvp: u32| {
            serde_json::json!({"athlete_id": a, "champion": c, "position": pos,
                "statistics": {"kills": k, "deaths": d, "assists": 5, "rating": r, "mvp": mvp}})
        };
        serde_json::json!({
            "blue_team_id": blue, "red_team_id": red, "blue_team_win": blue_win, "game_tick": 87_300,
            "blue_team": [player(1, "league_kalista", "Bottom", 10, 0, 82, u32::from(blue_win)), player(2, "league_thresh", "Support", 1, 2, 70, 0)],
            "red_team": [player(11, "league_jinx", "Bottom", 3, 6, 60, u32::from(!blue_win))],
            "blue_performance": {"total_kills": 24, "total_gold": 76178, "serpen_secured": 4, "epic_secured": 1, "first_serpen": true, "first_epic": true},
            "red_performance": {"total_kills": 6, "total_gold": 56411, "serpen_secured": 1, "epic_secured": 0, "first_serpen": false, "first_epic": false}
        })
        .to_string()
    }

    #[test]
    fn a_review_has_a_line_a_game_and_the_best_of_the_series() {
        let games = vec![parse_game(&game(78, 13, true)).unwrap(), parse_game(&game(13, 78, true)).unwrap()];
        assert_eq!(games[0].ticks, 87_300);
        assert_eq!(games[0].sides[0].players[0].kills, 10);
        let names = |a: u32| format!("选手{a}");
        let champs = |c: &str| if c == "league_kalista" { "卡莉丝塔".to_string() } else { c.to_string() };
        let (title, body) = review_news(78, ("Bark Knights", "Top Esports"), (1, 1), &games, true, &names, &champs);
        assert_eq!(title, "赛后复盘：Bark Knights 1:1 Top Esports");
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(
            lines[0],
            "第 1 局（蓝方，24 分 15 秒）胜：击杀 24:6，经济 76.2k:56.4k，小龙 4:1，大龙 1:0，先拿第一条小龙和大龙。MVP 选手1（卡莉丝塔 10/0/5）"
        );
        assert!(lines[1].starts_with("第 2 局（红方，24 分 15 秒）负：击杀 6:24"), "{}", lines[1]);
        // 你方最佳按平均评分：第一局的 1 号 8.2 最高（第二局我们在红方，是 11 号）
        assert_eq!(lines[2], "本系列赛你方最佳：选手1（1 局平均评分 8.2）");
        let (en, _) = review_news(78, ("A", "B"), (2, 1), &games, false, &names, &champs);
        assert_eq!(en, "Match review: A 2-1 B");
    }

    #[test]
    fn a_preview_says_what_it_knows() {
        let p = Preview {
            us: TeamInfo { name: "Bark Knights".into(), rank: Some((1, 10, 12, 2)), form: vec![true, true, false, true, true] },
            them: TeamInfo { name: "Top Esports".into(), rank: Some((3, 10, 9, 5)), form: vec![false, true] },
            best_of: 3,
            head_to_head: (5, 2),
            them_style: Some("开团型 · 偏爱坦克".into()),
            them_signature: vec!["卡莉丝塔".into(), "塞纳".into()],
            likely_bans: vec!["阿卡丽".into(), "劫".into(), "卡莎".into()],
            key_players: vec![("Faker".into(), "Mid".into())],
            strength: Some(0.623),
        };
        let (title, body) = preview_news(&p, true);
        assert_eq!(title, "赛前前瞻：Bark Knights vs Top Esports（BO3）");
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines[0], "积分榜：Bark Knights 第 1（12 胜 2 负），Top Esports 第 3（9 胜 5 负）");
        assert_eq!(lines[1], "近况：Bark Knights 胜胜负胜胜；Top Esports 负胜");
        assert_eq!(lines[2], "交手：这个存档里两队打过 7 局，Bark Knights 5 胜 2 负");
        assert_eq!(lines[3], "对方风格：开团型 · 偏爱坦克；招牌英雄：卡莉丝塔、塞纳");
        assert_eq!(lines[4], "对方可能先禁：阿卡丽、劫、卡莎");
        assert_eq!(lines[5], "对方关键选手：Faker（中单）");
        assert_eq!(lines[6], "只看双方选手，Bark Knights 赛前胜率约 62%（还没算 BP）");
        // 什么都不知道的时候也能写
        let bare_preview = Preview {
            us: TeamInfo { name: "A".into(), ..TeamInfo::default() },
            them: TeamInfo { name: "B".into(), ..TeamInfo::default() },
            ..Preview::default()
        };
        let (_, bare) = preview_news(&bare_preview, true);
        assert_eq!(bare, "近况：A 还没打过；B 还没打过\n交手：这个存档里两队还没交过手");
        assert!(preview_news(&p, false).1.contains("Form: Bark Knights W W L W W"));
    }
}
