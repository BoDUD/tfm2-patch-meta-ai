#!/usr/bin/env python3
"""Writes package/text/news.i18n: the balance patches' texts (2.8.0), merged into the game's news texts.

The game's patch notes give each champion a reason as codes of `patch.reason.*` texts; the balance patches' codes
(`balance::reason`) are the model's win rate (`pma_wr_<n>`, n 0..100), the change's power this patch and in all
(`pma_power_<m|p><n>`, `pma_sum_<m|p><n>`, 2.9.0: -11 is `m11`, n up to CAP) and the limits that held a change
(`pma_cap`, `pma_total_<CAP>`: 2.10.0's limit of 80; `pma_total` stays 2.9.0's 50 for the notes written then); an
unpicked buff (2.10.0, `balance::UNPICKED_STEP`) gives `pma_unpicked` instead of a win rate, power taken off at once
(2.11.0, `balance::Reset`) `pma_custom` (the player changed the champion's numbers) or `pma_skip` (`balance_skip`). A text
cannot carry a number of its own, so there is one per number. `patch.pma_none` (2.8.2) is what patch notes without a
champion change say (`server::UNCHANGED_TEXT`).

2.14.2 (the user: 「现在英雄平衡补丁的文字说明太僵硬 没有之前官方的那种文字的味道」): a change's reason starts with the game's
own `patch.reason.*` line (too_much_damage_2, buff_meta_1, ...: the game's texts, every language) and these texts follow it
as a short second line (the win rate, or too few games); the power is a line of the notes (`patch.pma_power_stat`, as a
stat: before -> after), so `pma_power_*` / `pma_sum_*` / `pma_cap` / `pma_total*` are read by older notes only. The texts
of the languages written here are short and plain, the way the game's own notes talk.

    python tools/news_i18n.py
"""
import json
import os

CAP = 80  # balance::CAP (2.10.0)
LEGACY_CAP = 50  # 2.9.0's, in `pma_total`: the notes it wrote at its limit keep saying 50

# (above the band, below it, near it, the round's limit, the overall limit); {n} is the win rate
TEXTS = {
    "zh-hans": (
        "近期胜率 {n}%，明显偏高。",
        "近期胜率 {n}%，明显偏低。",
        "近期胜率 {n}%。",
        "这已经是一个版本能调的上限，如果还不够，下个版本继续。",
        "累计调整已经到顶（±{cap}%）。",
    ),
    "zh-hant": (
        "近期勝率 {n}%，明顯偏高。",
        "近期勝率 {n}%，明顯偏低。",
        "近期勝率 {n}%。",
        "這已經是一個版本能調的上限，如果還不夠，下個版本繼續。",
        "累計調整已經到頂（±{cap}%）。",
    ),
    "en": (
        "Recent win rate: {n}%, clearly too high.",
        "Recent win rate: {n}%, clearly too low.",
        "Recent win rate: {n}%.",
        "That is as far as one patch goes; if it is not enough, the next one goes on.",
        "Its total adjustment has reached its limit (±{cap}%).",
    ),
    "ko": (
        "최근 승률 {n}%, 확실히 높습니다.",
        "최근 승률 {n}%, 확실히 낮습니다.",
        "최근 승률 {n}%.",
        "한 패치에서 조정할 수 있는 최대치입니다. 부족하면 다음 패치에서 이어서 조정합니다.",
        "누적 조정이 상한(±{cap}%)에 도달했습니다.",
    ),
    "ja": (
        "最近の勝率は{n}%と明らかに高め。",
        "最近の勝率は{n}%と明らかに低め。",
        "最近の勝率は{n}%。",
        "1回のパッチで調整できる上限です。足りなければ次のパッチでも続けます。",
        "累計の調整が上限（±{cap}%）に達しました。",
    ),
    "de": (
        "Die jüngste Siegquote liegt bei etwa {n} % und damit über dem Zielbereich von 48–52 %.",
        "Die jüngste Siegquote liegt bei etwa {n} % und damit unter dem Zielbereich von 48–52 %.",
        "Die jüngste Siegquote liegt bei etwa {n} %, nahe am Zielbereich von 48–52 %.",
        "Das ist die größte Änderung eines Patches; bleibt die Siegquote daneben, passt der nächste Patch erneut an.",
        "Die Gesamtanpassung hat die Grenze von ±{cap} % erreicht.",
    ),
    "fr": (
        "Son taux de victoire récent est d'environ {n} %, au-dessus de la cible de 48 à 52 %.",
        "Son taux de victoire récent est d'environ {n} %, en dessous de la cible de 48 à 52 %.",
        "Son taux de victoire récent est d'environ {n} %, proche de la cible de 48 à 52 %.",
        "C'est le changement maximal d'un patch ; si le taux de victoire reste décalé, le prochain patch l'ajustera encore.",
        "Son ajustement total a atteint la limite de ±{cap} %.",
    ),
    "es-ES": (
        "Su tasa de victorias reciente ronda el {n} %, por encima del objetivo del 48-52 %.",
        "Su tasa de victorias reciente ronda el {n} %, por debajo del objetivo del 48-52 %.",
        "Su tasa de victorias reciente ronda el {n} %, cerca del objetivo del 48-52 %.",
        "Es el cambio máximo de un parche; si la tasa de victorias sigue desviada, el próximo parche volverá a ajustarla.",
        "Su ajuste total ha alcanzado el límite del ±{cap} %.",
    ),
    "pt-BR": (
        "Sua taxa de vitória recente está em cerca de {n}%, acima da meta de 48–52%.",
        "Sua taxa de vitória recente está em cerca de {n}%, abaixo da meta de 48–52%.",
        "Sua taxa de vitória recente está em cerca de {n}%, perto da meta de 48–52%.",
        "Esta é a maior mudança de um patch; se a taxa de vitória continuar fora, o próximo patch ajusta de novo.",
        "O ajuste total chegou ao limite de ±{cap}%.",
    ),
    "it": (
        "Il suo tasso di vittoria recente è di circa il {n}%, sopra l'obiettivo del 48–52%.",
        "Il suo tasso di vittoria recente è di circa il {n}%, sotto l'obiettivo del 48–52%.",
        "Il suo tasso di vittoria recente è di circa il {n}%, vicino all'obiettivo del 48–52%.",
        "È la modifica massima di una patch; se il tasso di vittoria resta fuori, la patch successiva lo correggerà ancora.",
        "La modifica complessiva ha raggiunto il limite del ±{cap}%.",
    ),
    "ru": (
        "Недавний процент побед — около {n}%, выше целевых 48–52%.",
        "Недавний процент побед — около {n}%, ниже целевых 48–52%.",
        "Недавний процент побед — около {n}%, близко к целевым 48–52%.",
        "Это максимальное изменение за один патч; если процент побед останется вне цели, следующий патч снова его скорректирует.",
        "Суммарная корректировка достигла предела ±{cap}%.",
    ),
    "pl": (
        "Ostatni współczynnik zwycięstw wynosi około {n}%, powyżej celu 48–52%.",
        "Ostatni współczynnik zwycięstw wynosi około {n}%, poniżej celu 48–52%.",
        "Ostatni współczynnik zwycięstw wynosi około {n}%, blisko celu 48–52%.",
        "To największa zmiana w jednej łatce; jeśli współczynnik zwycięstw nadal odbiega, następna łatka znów go skoryguje.",
        "Łączna korekta osiągnęła limit ±{cap}%.",
    ),
    "tr": (
        "Son kazanma oranı yaklaşık %{n}, %48–52 hedefinin üzerinde.",
        "Son kazanma oranı yaklaşık %{n}, %48–52 hedefinin altında.",
        "Son kazanma oranı yaklaşık %{n}, %48–52 hedefine yakın.",
        "Bu, bir yamada yapılan en büyük değişiklik; kazanma oranı hedefin dışında kalırsa sonraki yama yeniden ayarlar.",
        "Toplam ayarlama ±%{cap} sınırına ulaştı.",
    ),
    "nl": (
        "Het recente winpercentage is ongeveer {n}%, boven het doel van 48–52%.",
        "Het recente winpercentage is ongeveer {n}%, onder het doel van 48–52%.",
        "Het recente winpercentage is ongeveer {n}%, dicht bij het doel van 48–52%.",
        "Dit is de grootste wijziging per patch; blijft het winpercentage afwijken, dan past de volgende patch het opnieuw aan.",
        "De totale aanpassing heeft de grens van ±{cap}% bereikt.",
    ),
    "vi": (
        "Tỷ lệ thắng gần đây khoảng {n}%, cao hơn mục tiêu 48–52%.",
        "Tỷ lệ thắng gần đây khoảng {n}%, thấp hơn mục tiêu 48–52%.",
        "Tỷ lệ thắng gần đây khoảng {n}%, gần mục tiêu 48–52%.",
        "Đây là mức thay đổi tối đa của một bản cập nhật; nếu tỷ lệ thắng vẫn lệch, bản cập nhật sau sẽ điều chỉnh tiếp.",
        "Tổng mức điều chỉnh đã chạm giới hạn ±{cap}%.",
    ),
    "th": (
        "อัตราชนะล่าสุดประมาณ {n}% สูงกว่าเป้าหมาย 48–52%",
        "อัตราชนะล่าสุดประมาณ {n}% ต่ำกว่าเป้าหมาย 48–52%",
        "อัตราชนะล่าสุดประมาณ {n}% ใกล้เป้าหมาย 48–52%",
        "นี่คือการปรับสูงสุดต่อหนึ่งแพตช์ หากอัตราชนะยังคลาดเคลื่อน แพตช์ถัดไปจะปรับอีก",
        "การปรับรวมถึงขีดจำกัด ±{cap}% แล้ว",
    ),
}
# what patch notes say when no champion changed
NONE = {
    "zh-hans": "本次版本没有英雄调整：各位英雄的表现都在合理范围内，或者刚调整过，还在观察新数值下的表现。",
    "zh-hant": "本次版本沒有英雄調整：各位英雄的表現都在合理範圍內，或者剛調整過，還在觀察新數值下的表現。",
    "en": "No champion changes this patch: everyone is performing within a healthy range, or was adjusted recently and is still being watched with the new numbers.",
    "ko": "이번 패치에는 챔피언 변경이 없습니다. 경기 수가 충분한 챔피언은 모두 목표 승률 구간(48~52%) 안에 있거나, 최근에 조정되어 새 수치로 치른 경기를 더 기다리고 있습니다.",
    "ja": "今回のパッチではチャンピオンの変更はありません。試合数が十分なチャンピオンはすべて目標勝率（48〜52%）の範囲内にあるか、最近調整されたばかりで新しい数値での試合を待っています。",
    "de": "In diesem Patch gibt es keine Champion-Änderungen: Alle Champions mit genügend Spielen liegen im Zielbereich von 48–52 % Siegquote oder wurden kürzlich angepasst und sammeln noch Spiele mit ihren neuen Werten.",
    "fr": "Aucun changement de champion dans ce patch : tous les champions ayant assez de parties sont dans la cible de 48 à 52 % de victoires, ou ont été modifiés récemment et accumulent encore des parties avec leurs nouvelles valeurs.",
    "es-ES": "Este parche no cambia ningún campeón: todos los campeones con suficientes partidas están dentro del objetivo del 48-52 % de victorias, o se ajustaron hace poco y aún acumulan partidas con sus nuevos valores.",
    "pt-BR": "Nenhum campeão mudou neste patch: todos os campeões com partidas suficientes estão dentro da meta de 48–52% de vitórias, ou foram ajustados há pouco e ainda acumulam partidas com os novos valores.",
    "it": "Nessun campione cambia in questa patch: tutti i campioni con abbastanza partite sono entro l'obiettivo del 48–52% di vittorie, oppure sono stati modificati di recente e stanno ancora accumulando partite con i nuovi valori.",
    "ru": "В этом патче нет изменений чемпионов: у всех чемпионов с достаточным числом игр процент побед в целевых 48–52%, либо их недавно изменили, и они ещё набирают игры с новыми показателями.",
    "pl": "W tej łatce nie zmieniono żadnego bohatera: wszyscy bohaterowie z wystarczającą liczbą gier mieszczą się w celu 48–52% zwycięstw albo zostali niedawno zmienieni i wciąż zbierają gry z nowymi wartościami.",
    "tr": "Bu yamada şampiyon değişikliği yok: yeterli maçı olan tüm şampiyonlar %48–52 kazanma oranı hedefinin içinde ya da yakın zamanda ayarlandı ve yeni değerleriyle maç biriktiriyor.",
    "nl": "Geen championwijzigingen in deze patch: alle champions met genoeg wedstrijden zitten binnen het doel van 48–52% winst, of zijn pas aangepast en verzamelen nog wedstrijden met hun nieuwe waarden.",
    "vi": "Bản cập nhật này không thay đổi tướng nào: mọi tướng có đủ số trận đều nằm trong mục tiêu tỷ lệ thắng 48–52%, hoặc vừa được điều chỉnh và còn đang chờ thêm trận với chỉ số mới.",
    "th": "แพตช์นี้ไม่มีการปรับแชมเปี้ยน: แชมเปี้ยนที่มีจำนวนเกมเพียงพอทุกตัวอยู่ในเป้าหมายอัตราชนะ 48–52% หรือเพิ่งถูกปรับและยังรอเกมด้วยค่าใหม่",
}
# the power of a change (2.9.0, `balance::reason`: pma_power_<m|p><n>, pma_sum_<m|p><n>): (this patch, this patch
# when the damage half is 0, in all); {s} {d} {h} {t} are signed numbers (+9, -11), d = s / 2 toward zero, h = s - d
POWER = {
    "zh-hans": ("这次整体强度 {s}%（攻击、法强 {d}%，生命 {h}%）。", "这次整体强度 {s}%（生命 {h}%）。", "累计 {t}%。"),
    "zh-hant": ("這次整體強度 {s}%（攻擊、法強 {d}%，生命 {h}%）。", "這次整體強度 {s}%（生命 {h}%）。", "累計 {t}%。"),
    "en": (
        "Power {s}% this patch ({d}% attack and magic power, {h}% health).",
        "Power {s}% this patch ({h}% health).",
        "{t}% power in all.",
    ),
    "ko": ("이번 패치 강도 {s}% (공격력·주문력 {d}%, 체력 {h}%).", "이번 패치 강도 {s}% (체력 {h}%).", "누적 강도 {t}%."),
    "ja": ("今回の強さ {s}%（攻撃力・魔力 {d}%、体力 {h}%）。", "今回の強さ {s}%（体力 {h}%）。", "累計の強さ {t}%。"),
    "de": (
        "Stärke {s} % in diesem Patch ({d} % Angriff und Fähigkeitsstärke, {h} % Leben).",
        "Stärke {s} % in diesem Patch ({h} % Leben).",
        "Insgesamt {t} % Stärke.",
    ),
    "fr": (
        "Puissance {s} % dans ce patch ({d} % d'attaque et de puissance magique, {h} % de vie).",
        "Puissance {s} % dans ce patch ({h} % de vie).",
        "{t} % de puissance au total.",
    ),
    "es-ES": (
        "Poder {s} % en este parche ({d} % de ataque y poder mágico, {h} % de vida).",
        "Poder {s} % en este parche ({h} % de vida).",
        "{t} % de poder en total.",
    ),
    "pt-BR": (
        "Poder {s}% neste patch ({d}% de ataque e poder mágico, {h}% de vida).",
        "Poder {s}% neste patch ({h}% de vida).",
        "{t}% de poder no total.",
    ),
    "it": (
        "Potenza {s}% in questa patch ({d}% di attacco e potere magico, {h}% di salute).",
        "Potenza {s}% in questa patch ({h}% di salute).",
        "{t}% di potenza in totale.",
    ),
    "ru": (
        "Сила {s}% в этом патче ({d}% атаки и силы умений, {h}% здоровья).",
        "Сила {s}% в этом патче ({h}% здоровья).",
        "Всего {t}% силы.",
    ),
    "pl": (
        "Siła {s}% w tej łatce ({d}% ataku i mocy umiejętności, {h}% zdrowia).",
        "Siła {s}% w tej łatce ({h}% zdrowia).",
        "Łącznie {t}% siły.",
    ),
    "tr": (
        "Bu yamada güç %{s} (saldırı ve büyü gücü %{d}, can %{h}).",
        "Bu yamada güç %{s} (can %{h}).",
        "Toplam güç %{t}.",
    ),
    "nl": (
        "Kracht {s}% in deze patch ({d}% aanval en magische kracht, {h}% leven).",
        "Kracht {s}% in deze patch ({h}% leven).",
        "{t}% kracht in totaal.",
    ),
    "vi": (
        "Sức mạnh {s}% trong bản cập nhật này ({d}% tấn công và sức mạnh phép, {h}% máu).",
        "Sức mạnh {s}% trong bản cập nhật này ({h}% máu).",
        "Tổng cộng {t}% sức mạnh.",
    ),
    "th": (
        "ความแข็งแกร่ง {s}% ในแพตช์นี้ (พลังโจมตีและพลังเวท {d}% พลังชีวิต {h}%)",
        "ความแข็งแกร่ง {s}% ในแพตช์นี้ (พลังชีวิต {h}%)",
        "ความแข็งแกร่งรวม {t}%",
    ),
}
# an unpicked buff's reason (2.10.0, `balance::reason`: `pma_unpicked`, before its power codes): too few games (fewer
# than balance::MIN_EVIDENCE) for its win rate to count, a small buff to bring it into play
UNPICKED = {
    "zh-hans": "近期场次不到 30 场，还看不出真实强度。",
    "zh-hant": "近期場次不到 30 場，還看不出真實強度。",
    "en": "Fewer than 30 games lately - too few to judge it yet.",
    "ko": "최근 경기 수가 30경기 미만이라 아직 판단하기 어렵습니다.",
    "ja": "最近の試合数は30未満で、まだ実力を判断できません。",
    "de": "In den letzten Patches kaum gewählt (weniger als 30 Spiele): vorerst eine kleine Verstärkung; sobald Spiele vorliegen, entscheidet die Siegquote.",
    "fr": "Rarement choisi lors des derniers patchs (moins de 30 parties) : un petit renforcement pour l'instant ; une fois les parties jouées, son taux de victoire décidera.",
    "es-ES": "Apenas elegido en los últimos parches (menos de 30 partidas): por ahora una pequeña mejora; cuando tenga partidas, decidirá su tasa de victorias.",
    "pt-BR": "Pouco escolhido nos últimos patches (menos de 30 partidas): por enquanto um pequeno buff; quando tiver partidas, a taxa de vitória decide.",
    "it": "Scelto raramente nelle ultime patch (meno di 30 partite): per ora un piccolo potenziamento; quando avrà partite, deciderà il suo tasso di vittoria.",
    "ru": "В последних патчах его почти не выбирали (меньше 30 игр): пока небольшое усиление; когда наберутся игры, всё решит процент побед.",
    "pl": "Rzadko wybierany w ostatnich łatkach (mniej niż 30 gier): na razie małe wzmocnienie; gdy zbierze gry, zdecyduje współczynnik zwycięstw.",
    "tr": "Son yamalarda nadiren seçildi (30 maçtan az): şimdilik küçük bir güçlendirme; maç verisi biriktiğinde kazanma oranına göre ayarlanacak.",
    "nl": "De laatste patches zelden gekozen (minder dan 30 wedstrijden): voorlopig een kleine buff; zodra er wedstrijden zijn, beslist het winpercentage.",
    "vi": "Ít được chọn trong vài bản cập nhật gần đây (dưới 30 trận): tạm thời tăng sức mạnh nhẹ; khi có đủ dữ liệu trận đấu sẽ điều chỉnh theo tỷ lệ thắng.",
    "th": "แทบไม่มีใครเลือกในช่วงหลายแพตช์ล่าสุด (ไม่ถึง 30 เกม) จึงเพิ่มพลังเล็กน้อยก่อน เมื่อมีข้อมูลการแข่งขันแล้วจะปรับตามอัตราชนะ",
}
# a champion whose numbers the player changed in the database (2.11.0, `balance::Reset::Edit`: `pma_custom`, before
# its power code): this mod's changes come off, it plays the player's numbers and is judged again from its games
CUSTOM = {
    "zh-hans": "你在数据库里改过它的数值，之前的平衡调整全部撤回，按你的数值来；之后用新数值打出的比赛会重新评估。",
    "zh-hant": "你在資料庫裡改過它的數值，之前的平衡調整全部撤回，按你的數值來；之後用新數值打出的比賽會重新評估。",
    "en": "You changed its numbers in the database: the earlier balance changes are taken back and yours apply; the games it plays with them decide what comes next.",
    "ko": "플레이어가 데이터베이스에서 수치를 바꿨습니다. 이 모드의 이전 조정은 취소되고 플레이어의 수치 그대로 적용되며, 새 수치로 치른 경기에 따라 다시 조정합니다.",
    "ja": "プレイヤーがデータベースで数値を変更しました。このModのこれまでの調整は取り消され、プレイヤーの数値どおりになります。新しい数値での試合をもとに改めて調整します。",
    "de": "Der Spieler hat seine Werte in der Datenbank geändert: Die bisherigen Anpassungen dieses Mods entfallen, es gelten genau die Werte des Spielers; die Spiele damit entscheiden über die nächsten Änderungen.",
    "fr": "Le joueur a modifié ses valeurs dans la base de données : les ajustements précédents de ce mod sont retirés et ses valeurs s'appliquent telles quelles ; les parties jouées avec elles décideront des prochains changements.",
    "es-ES": "El jugador cambió sus valores en la base de datos: se retiran los ajustes anteriores de este mod y se usan exactamente sus valores; las partidas con ellos decidirán los próximos cambios.",
    "pt-BR": "O jogador alterou os valores no banco de dados: os ajustes anteriores deste mod foram retirados e valem exatamente os valores do jogador; as partidas com eles decidem as próximas mudanças.",
    "it": "Il giocatore ha cambiato i suoi valori nel database: gli aggiustamenti precedenti di questa mod sono annullati e valgono esattamente i valori del giocatore; le partite giocate con essi decideranno le prossime modifiche.",
    "ru": "Игрок изменил его характеристики в базе данных: прежние правки этого мода сняты, действуют ровно значения игрока; дальнейшие изменения решат игры с ними.",
    "pl": "Gracz zmienił jego wartości w bazie danych: wcześniejsze zmiany tego moda zostały cofnięte i obowiązują dokładnie wartości gracza; o kolejnych zmianach zdecydują gry z nimi.",
    "tr": "Oyuncu veritabanında değerlerini değiştirdi: bu modun önceki ayarları kaldırıldı, tam olarak oyuncunun değerleri geçerli; sonraki değişikliklere bu değerlerle oynanan maçlar karar verecek.",
    "nl": "De speler heeft de waarden in de database aangepast: de eerdere aanpassingen van deze mod vervallen en precies de waarden van de speler gelden; de wedstrijden daarmee bepalen de volgende wijzigingen.",
    "vi": "Người chơi đã sửa chỉ số trong cơ sở dữ liệu: các điều chỉnh trước của mod này bị hủy, dùng đúng chỉ số của người chơi; các trận đấu với chỉ số mới sẽ quyết định lần điều chỉnh tiếp theo.",
    "th": "ผู้เล่นแก้ค่าสถานะในฐานข้อมูล: การปรับก่อนหน้านี้ของม็อดนี้ถูกยกเลิก และใช้ค่าของผู้เล่นตามเดิมทุกประการ การแข่งขันด้วยค่าใหม่จะเป็นตัวตัดสินการปรับครั้งต่อไป",
}
# a champion `balance_skip` lists (2.11.0, `balance::Reset::Skip`: `pma_skip`): never changed, this mod's changes off
SKIP = {
    "zh-hans": "按你的设置（balance_skip），它之前的调整全部撤回，之后也不会再动它。",
    "zh-hant": "按你的設定（balance_skip），它之前的調整全部撤回，之後也不會再動它。",
    "en": "As your settings ask (balance_skip), its earlier changes are taken back and it is left alone from now on.",
    "ko": "설정에서 조정하지 않도록 지정됨(balance_skip): 이 모드의 이전 조정은 취소되고 앞으로도 조정하지 않습니다.",
    "ja": "設定で調整しないよう指定されています（balance_skip）：このModのこれまでの調整は取り消され、今後も調整しません。",
    "de": "Als nicht zu ändern eingetragen (balance_skip): Die bisherigen Anpassungen dieses Mods entfallen, und er ändert ihn nicht mehr.",
    "fr": "Indiqué comme à ne pas modifier (balance_skip) : les ajustements précédents de ce mod sont retirés et il ne le modifiera plus.",
    "es-ES": "Marcado para no cambiarse (balance_skip): se retiran los ajustes anteriores de este mod y ya no lo cambiará.",
    "pt-BR": "Marcado para não ser alterado (balance_skip): os ajustes anteriores deste mod foram retirados e ele não será mais alterado.",
    "it": "Indicato come da non modificare (balance_skip): gli aggiustamenti precedenti di questa mod sono annullati e non verrà più modificato.",
    "ru": "Отмечен как неизменяемый (balance_skip): прежние правки этого мода сняты, больше он его не меняет.",
    "pl": "Oznaczony jako niezmieniany (balance_skip): wcześniejsze zmiany tego moda zostały cofnięte i nie będzie go już zmieniał.",
    "tr": "Değiştirilmeyecekler listesinde (balance_skip): bu modun önceki ayarları kaldırıldı ve artık değiştirilmeyecek.",
    "nl": "Aangemerkt als niet aan te passen (balance_skip): de eerdere aanpassingen van deze mod vervallen en hij wordt niet meer aangepast.",
    "vi": "Được đặt là không điều chỉnh (balance_skip): các điều chỉnh trước của mod này bị hủy và sẽ không điều chỉnh nữa.",
    "th": "ตั้งไว้ว่าไม่ต้องปรับ (balance_skip): การปรับก่อนหน้านี้ของม็อดนี้ถูกยกเลิก และจะไม่ปรับอีก",
}
# a theme patch (2.14.0, `theme`, `pma_theme_<category>`): a class of champions buffed together; categories are the
# game's ChampionCategoryV1 codes 0-5
THEME_NAMES = {
    "zh-hans": ["战士", "射手", "法师", "辅助", "刺客", "坦克"],
    "zh-hant": ["戰士", "射手", "法師", "輔助", "刺客", "坦克"],
    "en": ["fighters", "marksmen", "mages", "supports", "assassins", "tanks"],
}
THEME = {
    "zh-hans": "「{name}版本」到来！所有{name}一同加强，看看这个版本谁能脱颖而出。",
    "zh-hant": "「{name}版本」到來！所有{name}一同加強，看看這個版本誰能脫穎而出。",
    "en": "A patch for {name}! Every one of them gets stronger together - let's see who rises to the top.",
}
# the power as a line of the notes (2.14.2, `balance::power_row`: `patch.pma_power_stat` is its name, like 攻击力)
POWER_STAT = {
    "zh-hans": "整体强度",
    "zh-hant": "整體強度",
    "en": "Overall power",
    "ko": "전체 강도",
    "ja": "総合的な強さ",
    "de": "Gesamtstärke",
    "fr": "Puissance globale",
    "es-ES": "Fuerza general",
    "pt-BR": "Força geral",
    "it": "Potenza complessiva",
    "ru": "Общая сила",
    "pl": "Ogólna siła",
    "tr": "Genel güç",
    "nl": "Totale kracht",
    "vi": "Sức mạnh tổng thể",
    "th": "พลังโดยรวม",
}
# the game's languages (as package/text/ui.i18n); the ones without their own texts take English
LANGUAGES = ["ko", "en", "ja", "de", "vi", "pt-BR", "tr", "nl", "ru", "es-ES", "fr", "it", "zh-hans", "zh-hant", "pl", "th", "haw"]


def reasons(lang):
    above, below, near, cap, total = TEXTS.get(lang, TEXTS["en"])
    out = {}
    for n in range(101):
        text = above if n > 52 else below if n < 48 else near
        out[f"pma_wr_{n}"] = text.format(n=n)
    out["pma_cap"] = cap
    out["pma_total"] = total.format(cap=LEGACY_CAP)
    out[f"pma_total_{CAP}"] = total.format(cap=CAP)
    out["pma_unpicked"] = UNPICKED.get(lang, UNPICKED["en"])
    out["pma_custom"] = CUSTOM.get(lang, CUSTOM["en"])
    out["pma_skip"] = SKIP.get(lang, SKIP["en"])
    names = THEME_NAMES.get(lang, THEME_NAMES["en"])
    theme = THEME.get(lang, THEME["en"])
    for category, name in enumerate(names):
        out[f"pma_theme_{category}"] = theme.format(name=name)
    power, power_hp, in_all = POWER.get(lang, POWER["en"])
    signed = lambda v: f"{v:+d}"
    for v in range(-CAP, CAP + 1):
        if v == 0:
            continue
        code = ("m" if v < 0 else "p") + str(abs(v))
        d = int(v / 2)  # toward zero, as balance::buff splits it
        h = v - d
        text = power if d != 0 else power_hp
        out[f"pma_power_{code}"] = text.format(s=signed(v), d=signed(d), h=signed(h))
        out[f"pma_sum_{code}"] = in_all.format(t=signed(v))
    return out


def main():
    root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
    doc = {
        lang: {"patch": {"pma_none": NONE.get(lang, NONE["en"]), "pma_power_stat": POWER_STAT.get(lang, POWER_STAT["en"]), "reason": reasons(lang)}}
        for lang in LANGUAGES
    }
    path = os.path.join(root, "package", "text", "news.i18n")
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        json.dump(doc, f, ensure_ascii=False, indent=2)
        f.write("\n")
    print(path)


if __name__ == "__main__":
    main()
