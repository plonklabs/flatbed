#!/usr/bin/env python3
"""Wrap a content file (masthead, sections, figure data) into one self-contained page.

    assemble.py <content.html> <out.html> --title "Two to four words"

The stylesheet and the drawing library are inlined, so the page needs nothing
but Google Fonts.
"""
import argparse
import html
from pathlib import Path

HERE = Path(__file__).resolve().parent
FONTS = (
    "https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@400;600;800"
    "&family=IBM+Plex+Sans:wght@400;500;600&display=swap"
)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("content")
    p.add_argument("out")
    p.add_argument("--title", required=True)
    args = p.parse_args()
    css = (HERE / "press.css").read_text()
    js = (HERE / "diagram.js").read_text()
    body = Path(args.content).read_text()
    page = f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{html.escape(args.title)}</title>
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link rel="stylesheet" href="{FONTS}">
<style>
{css}</style>
<script>
{js}</script>
</head>
<body>
<div class="wrap">
{body}
</div>
</body>
</html>
"""
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(page)


if __name__ == "__main__":
    main()
