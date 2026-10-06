#!/usr/bin/env bash
# Builds patch_meta_ai.dll for Windows from Linux/WSL (target x86_64-pc-windows-gnu, needs the
# mingw-w64 linker: apt install gcc-mingw-w64-x86-64), runs the tests, and assembles the mod
# folder dist/patch_meta_ai/ and dist/patch_meta_ai-<version>.zip.
#   tools/build.sh            build + package
#   tools/build.sh --smoke    also load the built DLL under Wine (tools/dll-smoke)
set -euo pipefail
cd "$(dirname "$0")/.."
target=x86_64-pc-windows-gnu
rustup target add "$target" >/dev/null 2>&1 || true

cargo test --quiet
cargo build --release --target "$target"

version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
info_version=$(python3 -c "import json;print(json.load(open('package/mod.mod_info'))['version'])")
if [ "$version" != "$info_version" ]; then
    echo "Cargo.toml version $version != package/mod.mod_info version $info_version" >&2
    exit 1
fi

out=dist/patch_meta_ai
rm -rf "$out"
mkdir -p "$out"
cp "target/$target/release/patch_meta_ai.dll" package/mod.mod_info package/mod.override_info package/thumbnail.png "$out/"
# the page's texts, merged into the game's ui text (mod.override_info)
mkdir -p "$out/text"
cp package/text/ui.i18n "$out/text/"

if [ "${1:-}" = "--smoke" ]; then
    (cd tools/dll-smoke && cargo build --release --target "$target")
    smoke=$(mktemp -d)
    trap 'rm -rf "$smoke"' EXIT
    cp -r "$out" "$smoke/"
    wine=$(command -v wine64 || command -v wine || echo /usr/lib/wine/wine64)
    WINEDEBUG=-all "$wine" tools/dll-smoke/target/$target/release/dll-smoke.exe \
        "$(echo "Z:$smoke/patch_meta_ai/patch_meta_ai.dll" | tr '/' '\\')"
fi

python3 - "$version" <<'PY'
import os, sys, zipfile
version = sys.argv[1]
path = f"dist/patch_meta_ai-{version}.zip"
with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
    for root, _, files in sorted(os.walk("dist/patch_meta_ai")):
        for name in sorted(files):
            full = os.path.join(root, name)
            z.write(full, os.path.join("patch_meta_ai", os.path.relpath(full, "dist/patch_meta_ai")))
print(path)
PY
