# Patch Meta AI — 更聪明的 Ban/Pick + 自动英雄梯队（团战经理 2 / Teamfight Manager 2）

<img src="package/thumbnail.png" width="256" align="right" alt="Patch Meta AI">

按**当前游戏内版本**的真实胜负数据工作的原生 Mod（稳定版 Mod API，游戏 0.6 及以上）：

1. **Ban/Pick AI**：AI 选人时更倾向本版本真正赢球的英雄，不在弱势英雄上浪费 ban 位，优先 ban 掉"又强又热门"的英雄。
2. **自动梯队**：自动维护你队伍的 S/A/B/C/D 英雄梯队，版本一变、数据一多就跟着更新（游戏内第二天显示）。

数据全部来自你的存档：所有大会比赛记录、单排记录，以及球队新闻里的补丁公告。

## 安装

- **手动安装**：把 `patch_meta_ai` 文件夹（含 `patch_meta_ai.dll`、`mod.mod_info`、`thumbnail.png`）放进 `<游戏目录>\mods\`，进游戏在 Mod 菜单里启用（含代码的 Mod 会弹一次确认），重启游戏。
- 只有 Windows 版（`.dll`）。
- 不要和其他"按胜率改 ban/pick""自动设置梯队"的 Mod 同时启用，它们会互相覆盖。

## 设置：`settings.ini`

第一次运行时在 Mod 文件夹里自动生成，改完保存几秒内生效，不用重启。删掉会重新生成默认值。

| 段落 | 键 | 默认 | 作用 |
|---|---|---|---|
| `[features]` | `ban_pick` / `tier_list` | on / on | 两个功能各自的开关 |
| `[model]` | `baseline_games` | 20 | 每个英雄起步时的 50% 虚拟场次（越大越保守） |
| | `carry_games` | 40 | 上个版本最多带入多少场战绩作为起点 |
| | `changed_carry` | 0.5 | 补丁公告里被调整的英雄，带入比例 |
| | `patch_shift` | 0.02 | 被加强/削弱的英雄起点上调/下调的胜率 |
| | `solo_weight` | 0.5 | 一场单排算几场大会比赛 |
| | `reliability_games` | 30 | 置信度达到一半所需的场次 |
| | `reworked` | （空） | 本版本重做的英雄 id，忽略其上个版本数据 |
| `[draft]` | `pick_strength` / `ban_strength` | 1.0 / 0.8 | 对 AI 原有评分的影响力度 |
| | `edge_scale` | 0.5 | 胜率优势换算成影响力的尺度 |
| `[tiers]` | `min_games` | 10 | 证据场次少于此值的英雄不参与分级 |
| | `s` `a` `b` `c` | 10/20/40/20 | 各梯队所占百分比，其余为 D |
| | `unranked` | keep | 证据不足的英雄：`keep` 保持原梯队，`clear` 设为无梯队 |
| `[debug]` | `verbose` | off | 在 `diag.log` 里写更多细节 |

## 原理

每个英雄在当前版本的胜率用 Beta 分布估计：

- **起点**：`baseline_games` 场 50% 的虚拟对局，加上上个版本最多 `carry_games` 场的实际战绩。补丁公告里加强的英雄起点上调 `patch_shift`，削弱的下调；被调整的英雄只带入 `changed_carry` 的上版本数据，`reworked` 的完全不带。
- **更新**：本版本的大会比赛，加上单排对局乘以 `solo_weight`。

由此得到：

- **ban/pick 修正**：`tanh(logit(胜率) / edge_scale) × 置信度`，再乘以 `pick_strength` 或 `ban_strength`。ban 时，强势英雄还要乘以出场率系数（出场率是平均的 2 倍以上按 2 倍算，很少出场的按 0.5 倍）。
- **梯队**：按保守估计（胜率减一个标准误）排序，按比例切分。前 `s`% 为 S，以此类推。

## 出问题时看这两个文件

都在 Mod 文件夹里：

- `diag.log`：Mod 读到了什么、做了什么，包括每个存档第一次读到的数据格式（`[probe]` 行）、梯队有没有写进去。每次启动游戏重写，上一次的保留为 `diag.prev.log`。反馈问题时请附上它。
- `meta_table.txt`：每个英雄当前的估计胜率、场次、梯队和 ban/pick 修正值。

## 从源码构建

- Windows：装好 Rust 后在仓库根目录 `cargo build --release`，得到 `target\release\patch_meta_ai.dll`。
- Linux/WSL：`tools/build.sh`，需要 `rustup target add x86_64-pc-windows-gnu` 和 `gcc-mingw-w64-x86-64`。它会跑测试、交叉编译、打包出 `dist/patch_meta_ai/` 和 zip。
  - 加 `--smoke` 还会用 Wine 真正加载 DLL，跑一遍模拟的游戏流程（`tools/dll-smoke`）。
- `cargo test`：单元测试，加上走真实 C 接口的整局模拟（`tests/fake_host.rs`）。

`vendor/mod-api-stable` 是游戏自带的稳定版 Mod SDK（`mod-sdk-stable`，0.6.2，ABI 等级 9），版权归 TeamSamoyed。

## 致谢

"按胜率调整 ban/pick + 自动梯队"这个思路最早来自 yudra 的创意工坊 Mod「Win-Rate Ban/Pick AI + Champion Tiers」。那个 Mod 已停止更新，在新版游戏里会失效，原因见 [docs/background.md](docs/background.md)。本 Mod 是独立实现，代码、模型、设置和美术都是新的。

## English

Patch Meta AI is a native Teamfight Manager 2 mod (stable mod API, game 0.6+). It estimates every champion's win rate in the current in-game patch from your save: competition matches, solo-rank games, the previous patch's results and the patch notes. With that estimate it

1. nudges the draft AI's ban and pick scores;
2. keeps your team's S/A/B/C/D champion tier list up to date.

- Settings live in `settings.ini` next to the DLL and are hot-reloaded.
- `diag.log` and `meta_table.txt` show what the mod sees.
- Build with `cargo build --release` on Windows, or `tools/build.sh` from Linux.
- Inspired by yudra's discontinued "Win-Rate Ban/Pick AI + Champion Tiers"; independent implementation.
- MIT licensed. `vendor/mod-api-stable` is TeamSamoyed's SDK.
