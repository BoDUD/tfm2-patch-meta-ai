#!/usr/bin/env python3
"""Writes package/text/news.i18n: the balance patches' texts (2.8.0), merged into the game's news texts.

The game's patch notes give each champion a reason as codes of `patch.reason.*` texts; the balance patches' codes
(`balance::reason`) are the model's win rate (`pma_wr_<n>`, n 0..100) and the limits that held a change (`pma_cap`,
`pma_total`). A text cannot carry a number of its own, so there is one per win rate. `patch.pma_none` (2.8.2) is what
patch notes without a champion change say (`server::UNCHANGED_TEXT`).

    python tools/news_i18n.py
"""
import json
import os

CAP = 30  # balance::CAP

# (above the band, below it, near it, the round's limit, the overall limit); {n} is the win rate
TEXTS = {
    "zh-hans": (
        "近期胜率约 {n}%，高于 48%–52% 的目标区间。",
        "近期胜率约 {n}%，低于 48%–52% 的目标区间。",
        "近期胜率约 {n}%，在 48%–52% 的目标区间附近。",
        "本次已调到单版上限；如果胜率仍然偏离，下个版本会继续调整。",
        "累计调整已到 ±{cap}% 的上限。",
    ),
    "zh-hant": (
        "近期勝率約 {n}%，高於 48%–52% 的目標區間。",
        "近期勝率約 {n}%，低於 48%–52% 的目標區間。",
        "近期勝率約 {n}%，在 48%–52% 的目標區間附近。",
        "本次已調到單版上限；如果勝率仍然偏離，下個版本會繼續調整。",
        "累計調整已到 ±{cap}% 的上限。",
    ),
    "en": (
        "Its recent win rate is about {n}%, above the 48–52% target.",
        "Its recent win rate is about {n}%, below the 48–52% target.",
        "Its recent win rate is about {n}%, close to the 48–52% target.",
        "This is the most one patch changes; if the win rate stays off, the next patch adjusts it again.",
        "Its total adjustment has reached the ±{cap}% limit.",
    ),
    "ko": (
        "최근 승률이 약 {n}%로 목표 구간(48~52%)보다 높습니다.",
        "최근 승률이 약 {n}%로 목표 구간(48~52%)보다 낮습니다.",
        "최근 승률이 약 {n}%로 목표 구간(48~52%)에 가깝습니다.",
        "한 패치에서 조정할 수 있는 최대치입니다. 승률이 계속 벗어나면 다음 패치에서 다시 조정합니다.",
        "누적 조정이 ±{cap}% 상한에 도달했습니다.",
    ),
    "ja": (
        "最近の勝率は約{n}%で、目標の48〜52%を上回っています。",
        "最近の勝率は約{n}%で、目標の48〜52%を下回っています。",
        "最近の勝率は約{n}%で、目標の48〜52%に近い水準です。",
        "1回のパッチでの調整上限です。勝率が外れたままなら、次のパッチでも調整します。",
        "累計の調整が±{cap}%の上限に達しました。",
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
    "zh-hans": "本次版本没有英雄改动：场次足够的英雄，胜率都在 48%–52% 的目标区间内，或者刚调整过、还在等新数值下的比赛。",
    "zh-hant": "本次版本沒有英雄改動：場次足夠的英雄，勝率都在 48%–52% 的目標區間內，或者剛調整過、還在等新數值下的比賽。",
    "en": "No champion changes this patch: every champion with enough games is inside the 48–52% win-rate target, or was changed recently and is still gathering games with its new numbers.",
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
# the game's languages (as package/text/ui.i18n); the ones without their own texts take English
LANGUAGES = ["ko", "en", "ja", "de", "vi", "pt-BR", "tr", "nl", "ru", "es-ES", "fr", "it", "zh-hans", "zh-hant", "pl", "th", "haw"]


def reasons(lang):
    above, below, near, cap, total = TEXTS.get(lang, TEXTS["en"])
    out = {}
    for n in range(101):
        text = above if n > 52 else below if n < 48 else near
        out[f"pma_wr_{n}"] = text.format(n=n)
    out["pma_cap"] = cap
    out["pma_total"] = total.format(cap=CAP)
    return out


def main():
    root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
    doc = {lang: {"patch": {"pma_none": NONE.get(lang, NONE["en"]), "reason": reasons(lang)}} for lang in LANGUAGES}
    path = os.path.join(root, "package", "text", "news.i18n")
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        json.dump(doc, f, ensure_ascii=False, indent=2)
        f.write("\n")
    print(path)


if __name__ == "__main__":
    main()
