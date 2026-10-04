//! One game session driven through the mod's exported entry points the way the game does: the
//! client extension's `post_update` with a small save behind the data / scene / net tables, the
//! server extension's `handle_command` with the team record behind the server table, and the
//! draft hook - all through the C ABI, so the SDK shims compiled into the mod run too.
//!
//! Used by `tests/fake_host.rs` (linked in) and `tools/dll-smoke` (the built DLL, loaded with
//! LoadLibrary - run under Wine or on Windows).
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::mem::{size_of, zeroed};

use mod_api_stable::*;

struct World {
    scene: u32,
    team: usize,
    team_name: String,
    replays: BTreeMap<usize, String>,
    news: String,
    tiers: String,
    champions: Vec<String>,
    commands: Vec<(String, Vec<u8>)>,
    events: Vec<(String, Vec<u8>)>,
    taken: Vec<(String, Vec<u8>)>,
    log: Vec<String>,
}

unsafe fn world<'a>(s: *const c_void) -> &'a mut World {
    &mut *(s as *mut World)
}

unsafe fn text<'a>(p: *const u8, n: usize) -> &'a str {
    if n == 0 {
        return "";
    }
    std::str::from_utf8(std::slice::from_raw_parts(p, n)).unwrap()
}

unsafe fn bytes<'a>(p: *const u8, n: usize) -> &'a [u8] {
    if n == 0 {
        return &[];
    }
    std::slice::from_raw_parts(p, n)
}

unsafe fn write_out(data: &[u8], buf: *mut u8, cap: usize, out_len: *mut usize) -> bool {
    *out_len = data.len();
    if !buf.is_null() {
        std::ptr::copy_nonoverlapping(data.as_ptr(), buf, data.len().min(cap));
    }
    true
}

unsafe extern "C" fn host_log(_level: u32, msg: *const u8, len: usize) {
    eprintln!("[host log] {}", text(msg, len));
}

unsafe extern "C" fn scene_kind(s: *const c_void) -> u32 {
    world(s).scene
}

unsafe extern "C" fn player_team_id(s: *const c_void, out: *mut usize) -> bool {
    *out = world(s).team;
    true
}

unsafe extern "C" fn team_name(s: *const c_void, team: usize, buf: *mut u8, cap: usize, out: *mut usize) -> bool {
    let w = world(s);
    team == w.team && write_out(w.team_name.as_bytes(), buf, cap, out)
}

unsafe extern "C" fn record_ids(s: *const c_void, kind: u32, out: *mut usize, cap: usize) -> usize {
    let w = world(s);
    let ids: Vec<usize> =
        if kind == RecordKindV1::MatchReplay.code() { w.replays.keys().copied().collect() } else { vec![] };
    if !out.is_null() {
        for (i, id) in ids.iter().take(cap).enumerate() {
            *out.add(i) = *id;
        }
    }
    ids.len()
}

unsafe extern "C" fn record_get_json(
    s: *const c_void,
    kind: u32,
    id: usize,
    path: *const u8,
    path_len: usize,
    buf: *mut u8,
    cap: usize,
    out: *mut usize,
) -> bool {
    let w = world(s);
    let path = text(path, path_len);
    let doc = if kind == RecordKindV1::MatchReplay.code() && path.is_empty() {
        w.replays.get(&id).cloned()
    } else if kind == RecordKindV1::Team.code() && id == w.team && path == "news" {
        Some(w.news.clone())
    } else if kind == RecordKindV1::Team.code() && id == w.team && path == "champion_tiers" {
        Some(w.tiers.clone())
    } else {
        None
    };
    match doc {
        Some(doc) => write_out(doc.as_bytes(), buf, cap, out),
        None => false,
    }
}

unsafe extern "C" fn game_time(
    _s: *const c_void,
    y: *mut i32,
    mo: *mut u32,
    d: *mut u32,
    h: *mut u32,
    mi: *mut u32,
) -> bool {
    (*y, *mo, *d, *h, *mi) = (2026, 5, 1, 9, 0);
    true
}

unsafe extern "C" fn champion_count(s: *const c_void) -> usize {
    world(s).champions.len()
}

unsafe extern "C" fn champion_name_at(s: *const c_void, i: usize, buf: *mut u8, cap: usize, out: *mut usize) -> bool {
    match world(s).champions.get(i) {
        Some(name) => write_out(name.as_bytes(), buf, cap, out),
        None => false,
    }
}

unsafe extern "C" fn send_command(s: *mut c_void, c: *const u8, cl: usize, p: *const u8, pl: usize) -> bool {
    world(s).commands.push((text(c, cl).to_string(), bytes(p, pl).to_vec()));
    true
}

unsafe extern "C" fn take_events(s: *mut c_void) -> usize {
    let w = world(s);
    w.taken = std::mem::take(&mut w.events);
    w.taken.len()
}

unsafe extern "C" fn taken_event_at(
    s: *const c_void,
    index: usize,
    name_buf: *mut u8,
    name_cap: usize,
    name_len: *mut usize,
    payload_buf: *mut u8,
    payload_cap: usize,
    payload_len: *mut usize,
) -> bool {
    let Some((name, payload)) = world(s).taken.get(index) else { return false };
    write_out(name.as_bytes(), name_buf, name_cap, name_len) && write_out(payload, payload_buf, payload_cap, payload_len)
}

unsafe extern "C" fn team_get_json(
    s: *const c_void,
    team: usize,
    path: *const u8,
    path_len: usize,
    buf: *mut u8,
    cap: usize,
    out: *mut usize,
) -> bool {
    let w = world(s);
    team == w.team && text(path, path_len) == "champion_tiers" && write_out(w.tiers.as_bytes(), buf, cap, out)
}

/// The game's schema for `champion_tiers`: an object of tier names.
unsafe extern "C" fn team_set_json(
    s: *mut c_void,
    team: usize,
    path: *const u8,
    path_len: usize,
    json: *const u8,
    json_len: usize,
) -> bool {
    let w = world(s);
    if team != w.team || text(path, path_len) != "champion_tiers" {
        return false;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text(json, json_len)) else { return false };
    let valid = value
        .as_object()
        .is_some_and(|m| m.values().all(|v| matches!(v.as_str(), Some("S" | "A" | "B" | "C" | "D" | "NoTier"))));
    if valid {
        w.tiers = value.to_string();
    }
    valid
}

unsafe extern "C" fn emit_event(
    s: *mut c_void,
    _target_kind: u32,
    _target_id: usize,
    event: *const u8,
    event_len: usize,
    payload: *const u8,
    payload_len: usize,
) -> bool {
    let w = world(s);
    w.events.push((text(event, event_len).to_string(), bytes(payload, payload_len).to_vec()));
    w.log.push(format!("event {}", text(event, event_len)));
    true
}

fn match_json(id: usize, blue: &[&str], red: &[&str], blue_win: bool) -> String {
    let lanes = ["Top", "Jungle", "Mid", "Bottom", "Support"];
    let side = |names: &[&str]| {
        names
            .iter()
            .zip(lanes)
            .map(|(n, l)| format!(r#"{{"athlete":{id},"champion":"{n}","position":"{l}","kills":3}}"#))
            .collect::<Vec<_>>()
            .join(",")
    };
    format!(
        r#"{{"id":{id},"version":"2.4","seed":1234,"blue_team_win":{blue_win},"blue_team":[{}],"red_team":[{}],"frames":[[1,2,3]]}}"#,
        side(blue),
        side(red)
    )
}

pub type RequiredFn = unsafe extern "C" fn() -> u32;
pub type EntryFn = unsafe extern "C" fn(*const HostApiV1) -> *mut ModExportV1;

/// Runs the session; `dir` is the mod folder the entry point was told to use. Panics on the
/// first thing that is wrong.
pub fn run(required: RequiredFn, entry: EntryFn, dir: &std::path::Path) {

    let names = ["alpha", "bravo", "charlie", "delta", "echo", "fox", "golf", "hotel", "india", "juliet"];
    let mut w = World {
        scene: SceneKindV1::Title.code(),
        team: 7,
        team_name: "Samoyed Gaming".into(),
        replays: BTreeMap::new(),
        news: r#"[{"title_bind":["2.4"],"kind":"PatchNote","champion_patch_data":[["juliet",[{"is_buff":false}]]]}]"#.into(),
        tiers: r#"{"alpha":"C","zulu":"A"}"#.into(),
        champions: names.iter().map(|s| s.to_string()).collect(),
        commands: vec![],
        events: vec![],
        taken: vec![],
        log: vec![],
    };
    // 30 matches: the side with "alpha" wins, otherwise blue wins
    for id in 1..=30usize {
        let mut lineup: Vec<&str> = names.to_vec();
        lineup.rotate_left(id % 10);
        let (blue, red) = lineup.split_at(5);
        let blue_win = !red.contains(&"alpha");
        w.replays.insert(id, match_json(id, blue, red, blue_win));
    }

    unsafe {
        let mut host: HostApiV1 = zeroed();
        host.size = size_of::<HostApiV1>();
        host.host_abi_level = ABI_LEVEL;
        host.game_version = GameVersionV1 { major: 0, minor: 6, patch: 2 };
        host.log = Some(host_log);
        assert_eq!(required(), 1);
        let ex = &*entry(&host);
        assert_eq!(text(ex.mod_id_ptr, ex.mod_id_len), "patch_meta_ai");
        assert!(!ex.extension.is_null() && !ex.server_ext.is_null());
        assert_eq!(ex.draft_hooks_len, 1);
        let ext = &*ex.extension.vtable;
        let srv = &*ex.server_ext.vtable;
        let hook = *ex.draft_hooks_ptr;
        let hv = &*hook.vtable;

        let mut scene: SceneVtableV1 = zeroed();
        scene.size = size_of::<SceneVtableV1>();
        scene.kind = Some(scene_kind);
        let mut data: DataVtableV1 = zeroed();
        data.size = size_of::<DataVtableV1>();
        data.player_team_id = Some(player_team_id);
        data.team_name = Some(team_name);
        data.record_ids = Some(record_ids);
        data.record_get_json = Some(record_get_json);
        data.game_time = Some(game_time);
        data.champion_count = Some(champion_count);
        data.champion_name_at = Some(champion_name_at);
        let mut net: NetVtableV1 = zeroed();
        net.size = size_of::<NetVtableV1>();
        net.send_command = Some(send_command);
        net.take_events = Some(take_events);
        net.taken_event_at = Some(taken_event_at);
        let mut server: ServerVtableV1 = zeroed();
        server.size = size_of::<ServerVtableV1>();
        server.team_get_json = Some(team_get_json);
        server.team_set_json = Some(team_set_json);
        server.emit_event = Some(emit_event);

        let state = &mut w as *mut World as *mut c_void;
        let mut client = ClientCtxV1 {
            size: size_of::<ClientCtxV1>(),
            ui: std::ptr::null(),
            scene: &scene,
            asset: std::ptr::null(),
            save: std::ptr::null(),
            net: &net,
            state,
            data: &data,
            draw: std::ptr::null(),
        };
        let mut server_ctx = ServerCtxV1 { size: size_of::<ServerCtxV1>(), vtable: &server, state };

        let mut frame = |w_scene: u32| {
            (*(state as *mut World)).scene = w_scene;
            (ext.post_update.unwrap())(ex.extension.userdata, &mut client, 16_000);
            // the game hands commands to the server extension
            let commands = std::mem::take(&mut (*(state as *mut World)).commands);
            for (cmd, payload) in commands {
                let result = (srv.handle_command.unwrap())(
                    ex.server_ext.userdata,
                    &mut server_ctx,
                    cmd.as_ptr(),
                    cmd.len(),
                    payload.as_ptr(),
                    payload.len(),
                    0,
                    false,
                    7,
                    true,
                );
                assert_eq!(result, CommandResultV1::Handled.code());
            }
        };

        // on the title screen nothing happens
        frame(SceneKindV1::Title.code());
        assert!(w.log.is_empty());
        // a save is open: the 30 matches are read a few per frame (about 1 ms of reading per
        // frame), the model is rebuilt (every 5 s of real time), the tier list goes out, the
        // server writes it and answers, the client reads the answer
        let started = std::time::Instant::now();
        let mut frames = 0;
        while !(*(state as *mut World)).log.contains(&"event apply_tiers_result".to_string()) {
            assert!(started.elapsed().as_secs() < 30, "no tier list after {frames} frames");
            frame(SceneKindV1::InGame.code());
            frames += 1;
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        frame(SceneKindV1::InGame.code()); // the client takes the server's answer
        let tiers: serde_json::Value = serde_json::from_str(&w.tiers).unwrap();
        assert_eq!(tiers["alpha"], "S", "{tiers}");
        assert_eq!(tiers["zulu"], "A", "kept: not a champion of this save");
        assert!(tiers.as_object().unwrap().len() >= 10);
        assert_eq!(w.log, ["event apply_tiers_result"]);

        // the draft hook through the C ABI: alpha up, juliet (nerfed, losing) down
        let brief_names: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        let briefs: Vec<ChampionBriefV1> = brief_names
            .iter()
            .map(|n| ChampionBriefV1 { name: StrV1::from_str(n), ..Default::default() })
            .collect();
        let available: Vec<usize> = (0..briefs.len()).collect();
        let picks = [3usize];
        let mut ctx: DraftCtxV1 = zeroed();
        ctx.size = size_of::<DraftCtxV1>();
        ctx.phase = DraftPhaseV1::Pick.code();
        ctx.available_ptr = available.as_ptr();
        ctx.available_len = available.len();
        ctx.ally_pick_ptr = picks.as_ptr();
        ctx.ally_pick_len = picks.len();
        ctx.champion_briefs_ptr = briefs.as_ptr();
        ctx.champion_briefs_len = briefs.len();
        let score = |f: Option<unsafe extern "C" fn(*const c_void, *const DraftCtxV1, usize, f32, *mut f32) -> u32>, c: usize| {
            let mut out = 0.0f32;
            let code = (f.unwrap())(hook.userdata, &ctx, c, 1.0, &mut out);
            (code, out)
        };
        let (code, alpha) = score(hv.score_pick, 0);
        assert_eq!(code, DraftDecisionKindV1::Add.code());
        assert!(alpha > 0.3, "alpha pick {alpha}");
        let (code, alpha_ban) = score(hv.score_ban, 0);
        assert_eq!(code, DraftDecisionKindV1::Add.code());
        assert!(alpha_ban > 0.0);
        let (_, juliet) = score(hv.score_pick, 9);
        assert!(juliet < alpha, "juliet {juliet}");
        let (code, _) = score(hv.score_pick, 99);
        assert_eq!(code, DraftDecisionKindV1::Pass.code(), "unknown candidate");

        // the files a player looks at
        let table = std::fs::read_to_string(dir.join("meta_table.txt")).unwrap();
        assert!(table.contains("alpha") && table.contains("patch 2.4"), "{table}");
        let log = std::fs::read_to_string(dir.join("diag.log")).unwrap();
        assert!(log.contains("[probe] competition record #30"), "{log}");
        assert!(log.contains("tier list accepted"), "{log}");
        assert!(std::fs::read_to_string(dir.join("settings.ini")).unwrap().contains("[draft]"));

        // back to the title: the tables are dropped, the hook passes
        frame(SceneKindV1::Title.code());
        let (code, _) = score(hv.score_pick, 0);
        assert_eq!(code, DraftDecisionKindV1::Pass.code());
    }
}
