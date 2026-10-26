#!/usr/bin/env bash
# Screenshot a page in both themes, cut into 1000px slices to look at.
#   render.sh <page.html> <out-dir> [height] [width]
set -euo pipefail

page=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
out=$2
height=${3:-6000}
width=${4:-1200}
mkdir -p "$out"

chrome=""
for c in "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" google-chrome chromium chromium-browser; do
  if command -v "$c" >/dev/null 2>&1 || [ -x "$c" ]; then chrome=$c; break; fi
done
[ -n "$chrome" ] || { echo "no Chrome or Chromium found" >&2; exit 1; }

for theme in light dark; do
  # Blink's preferred scheme is an enum, dark 0 and light 1; forcing it renders a
  # page that only reads prefers-color-scheme, as well as one that reads ?theme=.
  scheme=$([ "$theme" = dark ] && echo 0 || echo 1)
  "$chrome" --headless=new --disable-gpu --hide-scrollbars --window-size="$width","$height" \
    --blink-settings=preferredColorScheme="$scheme" \
    --virtual-time-budget=4000 --screenshot="$out/$theme.png" "file://$page?theme=$theme" 2>/dev/null || true
  # Headless Chrome can exit non-zero on teardown after writing the file, so
  # the screenshot's presence is the check, not the exit code.
  [ -s "$out/$theme.png" ] || { echo "Chrome wrote no $theme screenshot" >&2; exit 1; }
done
# A probe taller than the capture: content below the capture's bottom means the page was cut.
"$chrome" --headless=new --disable-gpu --hide-scrollbars --window-size="$width","$((height + 800))" \
  --blink-settings=preferredColorScheme=1 \
  --virtual-time-budget=4000 --screenshot="$out/probe.png" "file://$page?theme=light" 2>/dev/null || true
[ -s "$out/probe.png" ] || { echo "Chrome wrote no probe screenshot" >&2; exit 1; }

python3 - "$out" "$height" <<'PY'
import sys
from pathlib import Path
try:
    from PIL import Image
except ImportError:
    raise SystemExit("render.sh needs Pillow: pip install pillow")

out, height = Path(sys.argv[1]), int(sys.argv[2])


def content_bottom(im):
    w, h = im.size
    bg = im.getpixel((w - 2, h - 2))
    bottom = h
    while bottom > 1 and all(im.getpixel((x, bottom - 1)) == bg for x in range(0, w, 20)):
        bottom -= 1
    return bottom


probe = out / "probe.png"
if content_bottom(Image.open(probe).convert("RGB")) > height:
    print(f"warning: the page runs past the {height}px capture; pass a larger [height]", file=sys.stderr)
probe.unlink()

for theme in ("light", "dark"):
    im = Image.open(out / f"{theme}.png").convert("RGB")
    w, _ = im.size
    bottom = content_bottom(im)
    for i, top in enumerate(range(0, bottom, 1000)):
        im.crop((0, top, w, min(top + 1000, bottom))).save(out / f"{theme}-{i}.png")
        print(out / f"{theme}-{i}.png")
PY
