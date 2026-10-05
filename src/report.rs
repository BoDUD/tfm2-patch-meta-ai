//! What the model knows, for the player: `meta_table.txt` (plain text, one line per champion)
//! and `meta_report.html` (open it in a browser: tiers, lane win rates, synergies, matchups,
//! players' champion pools and how well the model predicts). The page is one self-contained
//! file - the numbers are embedded as JSON and drawn by a small script, nothing is fetched.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::glm::sigmoid;
use crate::history::Role;
use crate::meta::{Backtest, Effect, Meta};
use crate::model::Tier;

/// Display names and who is who, from the game.
#[derive(Clone, Debug, Default)]
pub struct Labels {
    pub champions: HashMap<String, String>,
    pub athletes: HashMap<u32, String>,
    pub team_name: String,
    /// The player's own athletes (most recent line-up first).
    pub roster: Vec<u32>,
    pub date: Option<(i32, u32, u32)>,
}

impl Labels {
    pub fn champion(&self, id: &str) -> String {
        self.champions.get(id).cloned().unwrap_or_else(|| id.to_string())
    }

    fn athlete(&self, id: u32) -> String {
        self.athletes.get(&id).cloned().unwrap_or_else(|| format!("#{id}"))
    }
}

/// An effect in win-rate points at an even game (+0.05 = 50% -> 55%).
fn points(v: f32) -> f32 {
    sigmoid(v) - 0.5
}

fn pct(v: f32) -> f32 {
    (v * 1000.0).round() / 10.0
}

pub fn table(meta: &Meta, tiers: &HashMap<String, Tier>, labels: &Labels, summary: &str, bt: Option<&Backtest>) -> String {
    let mut rows: Vec<_> = meta.champions.iter().collect();
    rows.sort_by(|a, b| b.cautious().partial_cmp(&a.cautious()).unwrap_or(std::cmp::Ordering::Equal));
    let mut out = format!(
        "{} {} - patch {}\n{summary}\n{}\n\n\
         tier   = tier from the model (- = not enough recent games)\n\
         win%   = estimated win rate this patch with average team-mates and players, +- one sd\n\
         prev   = the same in the patch before\n\
         games  = this patch (competition) / kept patches; recent = tier evidence\n\
         lanes  = estimated win rate in each lane (Top Jungle Mid Bottom Support), - = never played there\n\n",
        crate::MOD_ID,
        crate::VERSION,
        meta.current,
        bt.map_or(String::new(), backtest_line),
    );
    out.push_str(&format!(
        "{:<22} {:>4} {:>12} {:>6} {:>11} {:>6} {:>5} {:>5}  {:<29} {}\n",
        "champion", "tier", "win%", "prev", "games", "recent", "pick", "ban", "lanes T/J/M/B/S", "patch"
    ));
    for c in rows {
        let lanes: Vec<String> = Role::ALL
            .iter()
            .map(|r| {
                if c.roles[r.index()].tally.games == 0 {
                    "-".to_string()
                } else {
                    format!("{:.0}", c.role_rate(*r) * 100.0)
                }
            })
            .collect();
        out.push_str(&format!(
            "{:<22} {:>4} {:>6.1}+-{:<4.1} {:>6} {:>11} {:>6.1} {:>4.0}% {:>4.0}%  {:<29} {}\n",
            crate::diag::clip(&labels.champion(&c.name), 22),
            tiers.get(&c.name).map_or("-", |t| t.as_str()),
            c.win_rate() * 100.0,
            (sigmoid(c.strength + c.sd) - c.win_rate()) * 100.0,
            c.previous.map_or("-".to_string(), |p| format!("{:.1}", sigmoid(p) * 100.0)),
            format!("{}/{}", c.current.games, c.window.games),
            c.evidence,
            c.pick_rate * 100.0,
            c.ban_rate * 100.0,
            lanes.join("/"),
            match &c.last_change {
                Some((v, d)) => format!("{} {v}", if *d > 0 { "buff" } else { "nerf" }),
                None => String::new(),
            }
        ));
    }
    out
}

pub fn backtest_line(bt: &Backtest) -> String {
    format!(
        "model check on the {} newest matches it was not fitted on: favourite won {:.0}%, Brier {:.3} (coin flip {:.3})",
        bt.games,
        bt.accuracy * 100.0,
        bt.brier,
        bt.coin_brier
    )
}

fn pair_rows(map: &HashMap<(u16, u16), Effect>, meta: &Meta, labels: &Labels, limit: usize) -> Vec<Value> {
    let mut list: Vec<(&(u16, u16), &Effect)> = map.iter().filter(|(_, e)| e.tally.games >= 3).collect();
    list.sort_by(|a, b| b.1.value.abs().partial_cmp(&a.1.value.abs()).unwrap_or(std::cmp::Ordering::Equal));
    list.truncate(limit);
    list.iter()
        .map(|((a, b), e)| {
            json!({
                "a": labels.champion(meta.names.name(*a)),
                "b": labels.champion(meta.names.name(*b)),
                "lift": pct(points(e.value)),
                "games": e.tally.games,
                "wr": e.tally.rate().map(pct),
            })
        })
        .collect()
}

pub fn html(meta: &Meta, tiers: &HashMap<String, Tier>, labels: &Labels, bt: Option<&Backtest>) -> String {
    let champions: Vec<Value> = meta
        .champions
        .iter()
        .filter(|c| c.window.games > 0 || c.current.games > 0)
        .map(|c| {
            json!({
                "name": labels.champion(&c.name),
                "tier": tiers.get(&c.name).map(|t| t.as_str()),
                "wr": pct(c.win_rate()),
                "sd": pct(sigmoid(c.strength + c.sd) - c.win_rate()),
                "prev": c.previous.map(|p| pct(sigmoid(p))),
                "games": c.current.games,
                "window": c.window.games,
                "raw": c.window.rate().map(pct),
                "pick": pct(c.pick_rate),
                "ban": pct(c.ban_rate),
                "lanes": Role::ALL.iter().map(|r| {
                    let e = c.roles[r.index()];
                    if e.tally.games == 0 { Value::Null } else { json!([pct(c.role_rate(*r)), e.tally.games]) }
                }).collect::<Vec<_>>(),
                "change": c.last_change.as_ref().map(|(v, d)| json!([v, d])),
                "now": c.patch_dir,
            })
        })
        .collect();
    let mut players = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for a in &labels.roster {
        if !seen.insert(*a) {
            continue;
        }
        let skill = meta.athletes.get(a).copied().unwrap_or_default();
        let mut pool: Vec<(u16, Effect)> =
            meta.mastery.iter().filter(|((x, _), e)| x == a && e.tally.games > 0).map(|((_, c), e)| (*c, *e)).collect();
        // what the player brings on each champion: the champion now plus their own mastery
        pool.sort_by(|x, y| {
            let vx = meta.by_id(x.0).map_or(0.0, |c| c.strength) + x.1.value;
            let vy = meta.by_id(y.0).map_or(0.0, |c| c.strength) + y.1.value;
            vy.partial_cmp(&vx).unwrap_or(std::cmp::Ordering::Equal)
        });
        players.push(json!({
            "name": labels.athlete(*a),
            "skill": pct(points(skill.value)),
            "games": skill.tally.games,
            "pool": pool.iter().take(12).map(|(c, e)| json!({
                "c": labels.champion(meta.names.name(*c)),
                "fit": pct(sigmoid(meta.by_id(*c).map_or(0.0, |x| x.strength) + e.value)),
                "mastery": pct(points(e.value)),
                "games": e.tally.games,
                "wr": e.tally.rate().map(pct),
            })).collect::<Vec<_>>(),
        }));
    }
    let data = json!({
        "mod": format!("{} {}", crate::MOD_ID, crate::VERSION),
        "team": labels.team_name,
        "date": labels.date.map(|(y, m, d)| format!("{y}-{m:02}-{d:02}")),
        "patch": meta.current,
        "patches": meta.versions,
        "matches": meta.matches,
        "current_matches": meta.current_matches,
        "solo": meta.solo_matches,
        "side": pct(points(meta.side)),
        "backtest": bt.map(|b| json!({"games": b.games, "accuracy": pct(b.accuracy), "brier": b.brier, "coin": b.coin_brier})),
        "champions": champions,
        "synergy": pair_rows(&meta.synergy, meta, labels, 200),
        "counter": pair_rows(&meta.counter, meta, labels, 300),
        "players": players,
    });
    // `</` cannot end the script early
    let data = data.to_string().replace("</", "<\\/");
    PAGE.replace("/*DATA*/null", &data)
}

const PAGE: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Patch Meta Report</title>
<style>
:root{--bg:#07080b;--panel:#161721;--line:#2a2c38;--text:#e8e8e8;--dim:#a3a9b6;--blue:#5b73ff;--red:#ef6471;--good:#4cc38a;--bad:#ef6471;
--S:#ff7a59;--A:#f2c14e;--B:#5b73ff;--C:#8a8fa3;--D:#5a5d6b}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--text);font:14px/1.45 system-ui,"Segoe UI","Microsoft YaHei",sans-serif}
header{padding:20px 24px 8px}h1{margin:0;font-size:22px}.sub{color:var(--dim);margin-top:4px}
nav{display:flex;gap:4px;padding:8px 24px;flex-wrap:wrap;position:sticky;top:0;background:var(--bg);z-index:2;border-bottom:1px solid var(--line)}
nav button{background:var(--panel);color:var(--dim);border:1px solid var(--line);border-radius:8px;padding:7px 14px;font:inherit;cursor:pointer}
nav button.on{color:var(--text);border-color:var(--blue)}
nav input{margin-left:auto;background:var(--panel);color:var(--text);border:1px solid var(--line);border-radius:8px;padding:7px 10px;font:inherit;min-width:180px}
main{padding:12px 24px 40px}section{display:none}section.on{display:block}
.cards{display:flex;gap:12px;flex-wrap:wrap;margin-bottom:14px}.card{background:var(--panel);border-radius:10px;padding:10px 14px;min-width:150px}
.card b{display:block;font-size:20px}.card span{color:var(--dim);font-size:12px}
.wrap{overflow-x:auto}table{border-collapse:collapse;width:100%;background:var(--panel);border-radius:10px;overflow:hidden}
th,td{padding:6px 10px;text-align:right;white-space:nowrap;border-bottom:1px solid var(--line)}th{color:var(--dim);font-weight:600;cursor:pointer;user-select:none;position:sticky;top:0;background:var(--panel)}
td:first-child,th:first-child,td.l,th.l{text-align:left}tr:hover td{background:#1d1f2c}
.tier{display:inline-block;width:22px;text-align:center;border-radius:5px;color:#07080b;font-weight:700}
.up{color:var(--good)}.down{color:var(--bad)}.dim{color:var(--dim)}.note{color:var(--dim);max-width:900px;margin:10px 0}
.pool{display:flex;flex-wrap:wrap;gap:6px}.chip{background:#1d1f2c;border-radius:6px;padding:3px 8px}
.player{background:var(--panel);border-radius:10px;padding:10px 14px;margin-bottom:10px}
</style></head><body>
<header><h1 id="title"></h1><div class="sub" id="sub"></div></header>
<nav id="nav"><input id="q" placeholder="Search / 搜索"></nav>
<main id="main"></main>
<script>
const D=/*DATA*/null;
const $=(t,a={},...k)=>{const e=document.createElement(t);for(const[n,v]of Object.entries(a)){if(n==='class')e.className=v;else if(n==='html')e.innerHTML=v;else e.setAttribute(n,v)}for(const c of k)e.append(c);return e};
const sign=v=>v==null?'':(v>0?'+':'')+v.toFixed(1);
const cls=v=>v>0.05?'up':v<-0.05?'down':'';
document.getElementById('title').textContent='Patch Meta — '+(D.team||'')+' · patch '+D.patch;
document.getElementById('sub').textContent=[D.mod,D.date,D.matches+' matches ('+D.current_matches+' this patch)',D.solo+' solo rank','patches '+D.patches.join(', ')].filter(Boolean).join(' · ');
const tabs=[['champ','Champions 英雄'],['syn','Synergy 配合'],['ctr','Matchups 克制'],['players','Players 选手'],['model','Model 模型']];
const nav=document.getElementById('nav'),main=document.getElementById('main'),q=document.getElementById('q');
const secs={};
for(const[id,label]of tabs){const b=$('button',{},label);b.onclick=()=>show(id);nav.insertBefore(b,q);secs[id]={b,s:main.appendChild($('section'))}}
function show(id){for(const[k,v]of Object.entries(secs)){v.b.classList.toggle('on',k===id);v.s.classList.toggle('on',k===id)}try{localStorage.setItem('pm-tab',id)}catch(e){}}
function table(cols,rows,sortIdx,desc=true){
 const t=$('table'),h=$('tr');let st={i:sortIdx,d:desc};
 cols.forEach((c,i)=>{const th=$('th',{class:c.l?'l':''},c.t);th.onclick=()=>{st.d=st.i===i?!st.d:true;st.i=i;draw()};h.append(th)});
 const head=$('thead',{},h),body=$('tbody');t.append(head,body);
 function draw(){const f=q.value.trim().toLowerCase();const rs=rows.filter(r=>!f||JSON.stringify(r).toLowerCase().includes(f));
  const k=cols[st.i].k;rs.sort((a,b)=>{const x=k(a),y=k(b);if(x==null)return 1;if(y==null)return -1;return (x<y?-1:x>y?1:0)*(st.d?-1:1)});
  body.replaceChildren(...rs.map(r=>$('tr',{},...cols.map(c=>$('td',{class:c.l?'l':'',html:c.h?c.h(r):String(c.k(r)??'')})))))}
 q.addEventListener('input',draw);draw();return $('div',{class:'wrap'},t)}
const tierOrder={S:5,A:4,B:3,C:2,D:1};
const lanes=['Top','Jg','Mid','Bot','Sup'];
secs.champ.s.append($('p',{class:'note'},'Win rate = estimated chance to win with average team-mates and players this patch (±1 sd). Lanes = the same in each lane. Raw = plain result over the kept patches. 胜率 = 本版本在队友和选手都是平均水平时的估计胜率；分路 = 各位置的估计胜率；原始 = 保留版本内的实际战绩。'),
 table([
  {t:'Champion',l:1,k:r=>r.name},
  {t:'Tier',k:r=>tierOrder[r.tier]??0,h:r=>r.tier?`<span class="tier" style="background:var(--${r.tier})">${r.tier}</span>`:'<span class="dim">-</span>'},
  {t:'Win %',k:r=>r.wr,h:r=>`${r.wr.toFixed(1)} <span class="dim">±${r.sd.toFixed(1)}</span>`},
  {t:'Δ prev',k:r=>r.prev==null?null:r.wr-r.prev,h:r=>r.prev==null?'':`<span class="${cls(r.wr-r.prev)}">${sign(r.wr-r.prev)}</span>`},
  {t:'Games',k:r=>r.games,h:r=>`${r.games} <span class="dim">/ ${r.window}</span>`},
  {t:'Raw %',k:r=>r.raw},
  {t:'Pick %',k:r=>r.pick},{t:'Ban %',k:r=>r.ban},{t:'Presence',k:r=>r.pick+r.ban,h:r=>(r.pick+r.ban).toFixed(1)},
  ...lanes.map((n,i)=>({t:n,k:r=>r.lanes[i]?r.lanes[i][0]:null,h:r=>r.lanes[i]?`${r.lanes[i][0].toFixed(0)} <span class="dim">${r.lanes[i][1]}g</span>`:'<span class="dim">-</span>'})),
  {t:'Last change',k:r=>r.change?r.change[0]:null,h:r=>r.change?`<span class="${r.change[1]>0?'up':'down'}">${r.change[1]>0?'▲':'▼'} ${r.change[0]}</span>`:''},
 ],D.champions,2));
secs.syn.s.append($('p',{class:'note'},'Lift = how much better (or worse) the pair does together than their own strengths predict, in win-rate points. Pairs with few games stay near 0. 提升 = 两人同队时比各自强度预期多赢（或少赢）的百分点；场次少的组合会被拉向 0。'),
 table([{t:'Champion',l:1,k:r=>r.a},{t:'With',l:1,k:r=>r.b},{t:'Lift',k:r=>r.lift,h:r=>`<span class="${cls(r.lift)}">${sign(r.lift)}</span>`},{t:'Games',k:r=>r.games},{t:'Raw %',k:r=>r.wr}],D.synergy,2));
secs.ctr.s.append($('p',{class:'note'},'Edge = how much the first champion beats the second beyond their strengths (positive = good matchup for the first). 优势 = 第一个英雄对上第二个时超出强度预期的胜率百分点（正 = 克制对方）。'),
 table([{t:'Champion',l:1,k:r=>r.a},{t:'Against',l:1,k:r=>r.b},{t:'Edge',k:r=>r.lift,h:r=>`<span class="${cls(r.lift)}">${sign(r.lift)}</span>`},{t:'Games',k:r=>r.games},{t:'Raw %',k:r=>r.wr}],D.counter,2));
{const s=secs.players.s;s.append($('p',{class:'note'},'Your players: their own strength, and their best champions now (champion strength this patch + their mastery). 你的选手：本人实力，以及当前最适合的英雄（英雄本版本强度 + 选手熟练度）。'));
 if(!D.players.length)s.append($('p',{class:'dim'},'No line-up seen yet. 还没有读到阵容。'));
 for(const p of D.players){s.append($('div',{class:'player'},$('div',{html:`<b>${p.name}</b> <span class="dim">· ${p.games} games · own strength <span class="${cls(p.skill)}">${sign(p.skill)}</span></span>`}),
  $('div',{class:'pool'},...p.pool.map(c=>$('span',{class:'chip',html:`${c.c} <b>${c.fit.toFixed(0)}%</b> <span class="dim">mastery ${sign(c.mastery)} · ${c.games}g</span>`})))))}}
{const s=secs.model.s,b=D.backtest;
 s.append($('div',{class:'cards'},
  $('div',{class:'card',html:`<b>${b?b.accuracy.toFixed(0)+'%':'—'}</b><span>favourite won (held-out games)<br>预测热门方获胜率（未参与拟合的比赛）</span>`}),
  $('div',{class:'card',html:`<b>${b?b.brier.toFixed(3):'—'}</b><span>Brier score (coin flip ${b?b.coin.toFixed(3):'0.250'}, lower is better)<br>Brier 分数（越低越好）</span>`}),
  $('div',{class:'card',html:`<b>${b?b.games:'—'}</b><span>held-out matches<br>检验场次</span>`}),
  $('div',{class:'card',html:`<b>${sign(D.side)}</b><span>blue side advantage (points)<br>蓝方优势（百分点）</span>`})),
  $('p',{class:'note'},'The model is fitted on all but the newest matches and scored on those it never saw. On a new save there are too few matches to check. 模型用除最新比赛外的数据拟合，再在没见过的最新比赛上打分；新存档比赛太少时无法检验。'))}
let start='champ';try{start=localStorage.getItem('pm-tab')||start}catch(e){}if(!secs[start])start='champ';show(start);
</script></body></html>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::tests::{simulate, NAMES};
    use crate::meta::{build, Inputs, Settings};

    #[test]
    fn writes_table_and_page() {
        let (names, games) = simulate(600, "1.1", |n| if n == "c" { 0.5 } else { 0.0 }, 2);
        let champions: Vec<String> = NAMES.iter().map(|s| s.to_string()).collect();
        let meta = build(
            &Inputs { games: &games, names: &names, champions: &champions, notes: &[], current: "1.1", warm: None },
            &Settings::default(),
        );
        let tiers: HashMap<String, Tier> = crate::meta::tiers(&meta, 10.0, [10.0, 20.0, 40.0, 20.0]).into_iter().collect();
        let mut labels = Labels { team_name: "Mods FC".into(), roster: vec![1, 2, 3], ..Default::default() };
        labels.champions.insert("c".into(), "Champion </script> C".into());
        let text = table(&meta, &tiers, &labels, "summary", None);
        let first = text.lines().skip_while(|l| !l.starts_with("champion ")).nth(1).unwrap();
        assert!(first.starts_with("Champion </script> C"), "the strongest first, by display name: {text}");
        let page = html(&meta, &tiers, &labels, Some(&Backtest { games: 10, accuracy: 0.6, brier: 0.2, log_loss: 0.6, coin_brier: 0.25 }));
        assert!(!page.contains("/*DATA*/null"));
        assert_eq!(page.matches("</script>").count(), 1, "names cannot close the script");
        assert!(page.contains("\"team\":\"Mods FC\""));
        assert!(page.contains("\"players\":[{"));
    }
}
