======================================================================
 Patch Meta AI 2.6.10 —— 智能 BP 教练 + 自动梯队
 团战经理 2 / Teamfight Manager 2（游戏 0.6 及以上，Windows）
======================================================================

【这是什么】
一个原生 Mod：读你存档里的真实比赛（大会比赛 + 单排），算出每个英雄在当前版本、
每个位置的强度，选手的实力和熟练度，英雄之间的配合与克制；在选人界面像教练一样
告诉你该禁谁、该选谁、为什么。还会改进 AI 队伍的 BP、自动维护你的英雄梯队。
英雄联盟英雄（league Mod 的英雄）另有内置的英雄联盟经验：官方位置、阵容属性
（前排/开团/保护/消耗/突进/后期/控制）、职业赛场梯队、知名的配合和克制，
存档比赛少的时候也能给出靠谱的建议，比赛多了由你存档的数据修正。

----------------------------------------------------------------------
【安装】
1. 解压后把整个 patch_meta_ai 文件夹放进  <游戏目录>\mods\
   （正确的位置是 <游戏目录>\mods\patch_meta_ai\patch_meta_ai.dll）
2. 进游戏，在 Mod 菜单里启用 Patch Meta AI（含代码的 Mod 会弹一次确认），重启游戏。
3. 读档后它会在后台读比赛记录，攒够 10 场比赛后选人界面开始显示建议。
不用任何设置，装上就能用。升级时直接覆盖整个文件夹即可（你的 settings.ini
和 history 文件夹会保留）。

----------------------------------------------------------------------
【选人界面（BP）】

■ 左下角：胜率面板
  · 大数字：你现在这套阵容的胜率。
  · 下面两行：双方的阵容类型和最大缺口，例如
      我方 开团 · 缺前排        敌方 消耗 · 伤害单一

■ 右下角：建议面板（四行）
  · 轮到你禁用：  禁 1 / 禁 2 / 禁 3 —— 最该禁的三个英雄
  · 对方在禁用：  敌禁? —— 对方最想禁你的三个（提前准备）
  · 选人阶段：    选 / 备 / 禁 —— 两个推荐选人 + 下一手最值得禁的
  · 第四行"敌可能"：对方下一手最可能选的三个（参考了这支队伍最近的常用英雄）
  任何禁用规则都能用（3+2 两轮禁用、一次性禁用都行）。

  禁用怎么算：不是"这个英雄多强"，而是"禁掉它，对方阵容会变差多少"。
  同位置有一样好的替代品就不值得禁；没有替代的、能打多个位置的才值得禁。
  我们自己先手能拿到的、我方选手拿手的，不会建议你去禁。

  每行右边是理由，常见的词：
    推演 xx%     3+2 规则下把这一手一直推演到 BP 结束，最终阵容的胜率（按它排序）
    无替代       这个位置没有同样好的英雄
    N路摇摆      能打多个位置
    克我方       克制我方已经选的英雄
    对方常用     对方队伍最近经常选它
    选手         对方（或我方）这个位置的选手很会玩它
    补前排 等    能补上阵容的缺口
    抢           选它的同时也让对方拿不到
    留后局       Fearless 系列赛里，后面还有对局，这种独一档的英雄先别用掉
    我方也想要   禁它我们自己也会损失

■ 英雄卡片上的数字
  · 禁用阶段：禁 x.x —— 禁掉它值多少胜率点（红色 = 值得禁）
  · 选人阶段：▲ / ▼ x.x —— 现在选它值多少胜率点（绿色 = 好，红色 = 差）

■ 估值明细
  鼠标放到英雄卡片上（或点一下），让下方的英雄信息栏显示它，
  选人格子最下面会出现一行明细：
    选 +3.1 = 中路 +2.0 · 克制 +0.5 · 阵容 +0.4 · 选手 +0.2 …
    禁 +1.2 = 对方损失 +1.5 · 我方损失 -0.3 · 无替代 · 对方常用

■ 其他
  · 敌方已选的英雄上标出它最可能打的位置和把握大小（五格条）。
  · 换位阶段，右下角改为显示五个英雄分给五名选手的最佳方式。
  · 用了 Smart Position Lock（智能位置锁定）时，被锁住的英雄不会出现在推荐里。

■ 教练托管
  BP 时点游戏的"教练托管"，你自己的队伍会完全照右下角面板的建议来禁和选
  （禁 1 / 选 那一行，推演和位置锁都算在内）。不影响其他队伍。
  不想要就在 settings.ini 里写 coach=off。
  F8 的 BP 复盘和 diag.log 的 [review] 行会写"教练托管决定了几手、实际照做了几手"；
  如果实际照做的少，说明别的 Mod 盖掉了它（见下面"地狱人机"）。

■ 胜率
  显示的胜率按"没参与拟合的最新比赛"校准过：模型以前偏自信，现在会往 50% 收一收，
  F8 模型页写着校准系数。

----------------------------------------------------------------------
【其他界面】
· 战术界面：选完人后，值得改的战术选项上标 ★ 和收益。
· 首发界面：推荐五名首发（标 ★ 和位置）。
· "版本分析"页：英雄 / 组合 / 选手 / 补丁变化 / 模型，可按位置筛选、按列排序，
  选手页有训练建议。经典界面在左侧菜单；新界面（游戏 0.7.0）在顶部标题栏右侧的按钮；
  主机布局（手柄）没有这个入口。
· 自动梯队：自动维护你队伍的 S/A/B/C/D 英雄梯队（游戏里换日后更新）。
· Mod 文件夹里的 meta_report.html（用浏览器打开）：全部数字，可排序、搜索。

【F8 面板（任何界面都能按）】
按 F8 打开，再按 F8 翻下一页，最后一页后关闭；Esc 直接关闭。
  1. 梯队榜
  2. 对手侦察：下一个对手每名选手的拿手英雄、对方常用英雄、建议禁用
  3. 我的选手：每名选手当前最好的英雄
  4. 模型：比赛数、准确度检验、英雄联盟经验的权重、属性先验（哪几项属性最值钱）、
     胜率校准系数、各种阵容缺口值多少胜率
  5. BP 复盘：上一次 BP 的每一手 —— 哪一方、选还是禁、当时对我方的价值、
     当时的建议、之后的胜率，以及你照建议走了几手

----------------------------------------------------------------------
【设置（可选）】
第一次运行后 Mod 文件夹里会生成 settings.ini，用记事本改，保存几秒内生效，
不用重启游戏。删掉它会重新生成默认设置。常用的几项：

  [features]
  ban_pick=auto      AI 队伍的 BP 用本 Mod 的估值（auto = 开，除非装了接管 AI 选人的 Mod）；off = 关
  tier_list=auto     自动梯队（auto = 开，除非装了写梯队的 Mod）；off = 关
  coach=on           教练托管时你自己的队伍按建议 BP；off = 关
  ai_draft=off       困难 AI：on = 所有 AI 队伍都完全按本 Mod 的建议禁和选（和面板同一套
                     排序，含 3+2 推演），盖过地狱人机等其他 BP Mod

  [model]
  lol_prior=auto     英雄联盟经验的分量：auto = 存档比赛够多后（约 150 场大会比赛）
                     自动检验 0 / 0.5 / 1 / 2 哪个最准；0 = 不用；也可以直接填 0～3
  stats=auto         英雄从自身属性出发（攻击、法强、生命、护甲、魔抗、移速及成长）：
                     auto = 用最新的比赛检验开 / 关哪个更准；on / off

  [screen]
  draft_overlay=on   选人界面的胜率和建议（以及战术、首发界面的推荐）
  grid_values=on     英雄卡片上的数字
  panel_opacity=60   两个面板背景的不透明度（0～100）；面板在屏幕最下面一条的两角，不挡选人位，
                     那一边选满五个后背景再降到三分之一
  lane_tags=on       敌方英雄的位置推断
  meta_page=on       "版本分析"页（经典界面左侧菜单 / 新界面标题栏）

其余设置的说明写在 settings.ini 里每一项的上面。

----------------------------------------------------------------------
【和其他 Mod 一起用】
· Smart Position Lock（智能位置锁定）：推荐自动跳过它锁住的英雄，可以一起用。
  困难 AI 和教练托管直接决定选谁，会按锁位置自己的位置表和规则只选排得下位置的英雄；
  英雄能打哪些位置也以锁位置的设置为准。
· Bows' Drafter's Toolbox：可以一起用，梯队自动交给它。
· Bows' Terminator Draft AI 等接管 AI 选人的 Mod：AI 的 BP 自动让给它，其他功能照常。
· 地狱人机（Hell Draft AI）：可以一起用。
    - AI 队伍的 BP 交给它；本 Mod 会按它的方式预测对方：敌可能、敌禁?、3+2 推演里
      对方都按它的偏好（AI 队伍常选常禁的、它自己统计胜率高的）来选和禁。
    - 它的"我方 AI"（ally_draft_ai）开着时，它也会接管你的教练托管。本 Mod 的教练托管
      排在最后决定，正常会以本 Mod 为准；如果 BP 复盘显示"照做的少"，在地狱人机的
      配置器（F7）里把我方 AI 关掉即可。

【出问题时】
把 Mod 文件夹里的 diag.log（以及 diag.prev.log）发给作者。
F9 可以把当前界面的结构写进 ui_dump_*.txt，界面显示不对时一起附上。
history 文件夹是各存档比赛的备份（游戏会删掉很旧的比赛），不要删。

======================================================================
 English (short)
======================================================================
Patch Meta AI reads your save's matches and fits one model of the meta: champion
strength per patch and lane, players' strength and mastery, synergies and counters.
League of Legends champions (the "league" mod) start from built-in League knowledge
(official lanes, composition traits, competitive tiers, known combos and counters).

Install: put the patch_meta_ai folder into <game>\mods\, enable it in the Mod menu,
restart the game. Advice shows once the save has 10 matches.

Ban/pick screen: bottom left - your win chance and both sides' composition style and
gaps. Bottom right - on your ban turn the three best bans (valued by how much worse
the enemy's draft gets without the champion), on theirs the bans they want against
you, in the pick phase the best picks; a fourth row with the enemy's likely next picks.
In a 3+2 draft the candidates are played out to the end of the draft ("推演 xx%").
Cards show what banning / picking each champion is worth. Hover a card for a value
breakdown above the info panel. Coach delegation bans and picks exactly what the panel
advises (coach=on), Hell Draft AI or not; with Hell Draft AI the enemy's likely picks,
bans and the lookahead follow how it drafts. Champions start from their stats
(stats=auto) and win chances are calibrated on held-out matches.

F8 anywhere: tier list, scouting, your players, the model, and a review of your last
draft. Settings: settings.ini in the mod folder (hot-reloaded). Problems: send diag.log.
