---
name: architecture-diagram
description: Draw architecture and sequence diagrams as Plonk Press figures — the colour roles, the shared stylesheet and drawing library, and the render check before a page is shown.
---

# /architecture-diagram — figures drawn as data in Plonk Press

How a page explains a system with pictures: a structure figure for what runs
where, and a sequence figure for each scenario that matters. Every figure uses
one design system, and the drawing is done by a shared library from data, so a
page costs its content and nothing else. Mermaid and other auto-layout tools
are not used for these pages: they place boxes and lines where the algorithm likes, and the
result reads as a tangle rather than an explanation.

## The design system

The figures are drawn in Plonk Press, the look every page the team publishes
shares. `assets/press.css` is its one stylesheet, inlined into every page by
`assets/assemble.py`: the Press tokens for light and dark, the page's classes
and the SVG vocabulary. `/artifact` owns the page's order and classes; this
skill owns the figure half. A page adds no colours of its own.

**A figure sits on paper in both themes.** The `.sheet` is `--paper` with ink
type, a 2px ink box rule in light and none in dark, so a figure's colours are
the same whichever theme the page is read in, and only the ground around it
changes.

**Colour carries meaning, never decoration.** A box's fill says who owns it,
a full-green band down its left edge says it is new, and a line's stroke says
what kind of call it is. Every box wears a 2px ink outline and every arrow is
ink, except the one kind that is new:

| Token or stroke | Meaning |
|---|---|
| `--plat` (cream) | a platform namespace or anything shared between users |
| `--user` (paper, solid 2.5px ink outline) | a user's own namespace, and stores that hold their data |
| `--new` / `--new-tint` (green, green tint) | a piece this proposal adds: the tint, the green edge and a `NEW` tag |
| `--paper` | a service, the cluster, anything outside it, a person |
| ink, solid (`int`) | a message or a call |
| ink, dotted (`ctl`) | control: scaling, scheduling, the platform acting on a workload |
| green, solid (`tool`) | a call inside the proposal's new contract |
| ink, heavy (`db`) | a write to a ledger or a database |
| caption, dashed (`wire`) | watching, reading, a reply, or anything outside the cluster |
| ochre tint with an ochre edge | a sequence note: time passing, a state, a wait |

Red never appears in a figure: in Press it means a person is wanted or
something is broken, and a figure shows neither. Grey is a parked thing.

**Type.** JetBrains Mono for everything the figure names: kind tags in heavy
tracked uppercase (`NAMESPACE`, `DATABASE`, `POD`), names and line labels as
written, with ligatures off so `-<` stays two characters. IBM Plex Sans for
prose inside a figure: a box's `sub`, a sequence step, a note.

**A figure's anatomy.** The page around it is `/artifact`'s. Each figure is a
`section.fig`: a ruled `h2` that opens with `<span class="fig-no">Fig. N</span>`
and states the figure's claim, a paragraph saying what to notice, then a
`.sheet` holding the `.scroll`, the `<svg class="d" id="…">` and a `.key`
listing only the box and line kinds that figure uses.

## The library

`assets/diagram.js` exposes `Diagram.arch(id, spec)` and
`Diagram.seq(id, parts, steps, opts)`. `assets/example.html` is a complete
content file using both — start from it.

**Structure figures** — `Diagram.arch`. The viewBox is 1190 wide; give
`height`. `boxes` are listed outermost first, each `{ kind, x, y, w, h, … }`:

| kind | Draws | Fields |
|---|---|---|
| `cluster` | the outer boundary | `name`, `note` (right-aligned) |
| `ns` | a namespace: dashed cream for `plat`, solid-ruled paper for `user` | `name`, `tone: "plat" \| "user"`, `tag` |
| `pod` | a pod, dotted, unfilled | `name`, `tag` |
| `proc` | a process inside a pod, always drawn new | `name`, `tag` |
| `svc` | a service or session | `name`, `sub`, `isNew` |
| `db` | a ledger or database | `tag`, `name`, `sub`, `note`, `caption` |
| `chip` | a subject, stream or key inside a service | `name`, `isNew` |
| `ext` | outside the cluster, dashed | `tag`, `name`, `sub`, `note`, `caption` |
| `pill` | a person | `name` |

`lines` are `{ k, d, label, at: [x, y], anchor, rotate, both, dash, end }`,
with `k` one of `int ctl tool db wire` and `d` an SVG path of `M`, `H` and `V`
segments only. `dash` dashes any kind but `ctl`, whose dotted stroke is its
identity, and `wire` is always dashed. `labels` are free text `{ x, y, text, cls, anchor }`, where `text` may be
a list of `[words, class]` spans. Lines are drawn over boxes and under every
text, and each text gets a halo in the fill it sits on, so a line passing under
a label never cuts through it.

**Sequence figures** — `Diagram.seq`. `parts` are the lifelines left to right,
`{ id, tag, label, kind }` with `kind` one of `user` (a pill), `new` (green
tint and edge), `store` (paper, solid-ruled) or the default paper service box. `steps` are `{ from, to, text, k, reply }` (a
`reply` is dashed, except a `ctl` step, which is always dotted; `from === to`
draws a self-call) or `{ note: [fromId, toId],
text }`. Steps are numbered automatically in ink discs, and the figure fits a 980-wide sheet
for up to eight parts.

## Steps

1. **Decide the figures before drawing.** One structure figure for what runs
   where, and one sequence per scenario the reader must follow — the normal
   path first, then each case that changes the picture (asleep, long work, a
   timer). Keep a sequence under about sixteen steps; split it otherwise.
2. **Lay the structure out on paper first.** Rows by layer (outside, platform,
   messaging, the user's namespace, storage), so most lines run vertically.
   Use a 10-unit grid, leave at least 40 units between siblings a labelled line
   runs between, end a line 2 units short of the box edge it points at, and
   route lines with `H`/`V` segments. A line never crosses a box it does not
   connect to; move the box or route around it.
3. **Write the content file** — masthead, figure captions, keys, and one script
   of `Diagram` calls — starting from `/artifact`'s `assets/page.html` for the
   page and `assets/example.html` for the figure calls. Give every figure a
   `title` that says in a sentence what it shows.
4. **Assemble**: `python3 assets/assemble.py content.html page.html --title "…"`.
   The page is self-contained apart from Google Fonts (JetBrains Mono and IBM
   Plex Sans).
5. **Render and look** at every slice in both themes before anyone else sees
   it: `assets/render.sh page.html <out-dir> [height] [width]` writes
   `light-N.png` and `dark-N.png` (1200 wide unless `width` says otherwise); a page taller than `height` (6000 by default) is cut, and
   the script warns when it was. It needs Chrome or Chromium and
   Pillow. Look for a label touching a line or a box edge, a label whose halo
   shows as a patch of another fill (its anchor point sits on a box edge;
   move it a few units into the fill it reads on), text clipped at a sheet's
   right edge, an arrow pointing the wrong way, a key listing a kind
   the figure does not use, and boxes in a sequence header that touch. Fix the
   data and render again; a page is shown only after a clean pass.
6. **Keep and publish** the assembled page where and how `/artifact` says.
   The figures stay `Diagram` data inside the assembled page, after the
   inlined stylesheet and library, so the next change edits that data in the
   one file and no content file is kept beside it. A change to the stylesheet
   or library reaches a kept page by re-assembling it from its own
   `<div class="wrap">` body, as long as that body uses the classes the
   stylesheet still carries. A page written against classes it no longer
   carries keeps the copy it inlines and is not re-assembled; it is rewritten
   in the current classes the next time its content changes.
