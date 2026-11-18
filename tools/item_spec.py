"""Writes src/ui/item_spec.rs: the game's templates for an item's numbers in its item page (2.12.1).

    python tools/item_spec.py [<game folder>]      (default: the Steam folder of Teamfight Manager 2)

The game shows a 4+6 item's numbers in its item page (游戏百科 -> 装备信息, the strategy screen's item popup) one line a
number, from `asset/base/text/item` -> `<language>.spec.<field>` (e.g. 攻击力 +{Value}). `ui::item_view` finds those
lines by these templates, in any language. Reads `bundle.game_data` (count u32, then per entry: type, path, data, each
u32-length-prefixed). Run it again when a game update changes the templates.
"""
import json
import os
import struct
import sys

GAME = sys.argv[1] if len(sys.argv) > 1 else r"D:\steam\steamapps\common\Teamfight Manager2"
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "src", "ui", "item_spec.rs")


def read(bundle, want_path, want_type):
    with open(bundle, "rb") as f:
        (count,) = struct.unpack("<I", f.read(4))
        for _ in range(count):
            (n,) = struct.unpack("<I", f.read(4))
            etype = f.read(n).decode("utf-8", "replace")
            (n,) = struct.unpack("<I", f.read(4))
            path = f.read(n).decode("utf-8", "replace")
            (size,) = struct.unpack("<I", f.read(4))
            if path == want_path and etype == want_type:
                return f.read(size)
            f.seek(size, 1)
    sys.exit(f"{want_path} ({want_type}) not in {bundle}")


text = json.loads(read(os.path.join(GAME, "bundle.game_data"), "asset/base/text/item", "i18n").decode("utf-8-sig"))
fields = {}
for lang in sorted(text):
    for field, template in (text[lang].get("spec") or {}).items():
        if isinstance(template, str) and "{Value}" in template and template not in fields.setdefault(field, []):
            fields[field].append(template)


def rust(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


lines = [
    "//! The game's templates for an item's numbers in its item page, every language's (`asset/base/text/item` ->",
    "//! `<language>.spec.<field>`, game 0.7.3). Written by `tools/item_spec.py` - run it again when a game update changes",
    "//! them. Each: the item's number field, and the lines it can make (`{Value}` = the number).",
    "",
    "pub const SPEC: &[(&str, &[&str])] = &[",
]
for field in sorted(fields):
    lines.append(f"    ({rust(field)}, &[{', '.join(rust(t) for t in fields[field])}]),")
lines.append("];")
open(OUT, "w", encoding="utf-8", newline="\n").write("\n".join(lines) + "\n")
print(f"{OUT}: {len(fields)} fields, {sum(len(v) for v in fields.values())} templates from {len(text)} languages")
