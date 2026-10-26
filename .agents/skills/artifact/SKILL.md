---
name: artifact
description: Write an HTML page the team publishes — a design page, a proposal, a report — in Plonk Press, with the page structure, the writing rules, the render check and where the page is kept.
---

# /artifact — a page the team publishes, in Plonk Press

How any HTML page written for a person to read is made: an epic's design page,
a proposal put to the owner, a report on what happened. Every such page speaks
one look, Plonk Press, and follows one order, so a reader who has read one
knows where to find things in the next. The page is one self-contained file.

`/architecture-diagram` owns what this skill does not: the figures, the
`Diagram` library that draws a structure or a sequence as data, and the
stylesheet and assembler every page is built with.

## Plonk Press

Press is a broadsheet: a masthead, ruled sections, cream boxes set on a ground,
heavy mono type for what is named and plain sans for what is said. It is one
stylesheet, `.agents/skills/architecture-diagram/assets/press.css`, carrying
the page's classes and the figures' vocabulary under one set of names; the
assembler inlines it, and a page adds no colours of its own.

| Token | Light | Dark | For |
|---|---|---|---|
| `--ground` | `#efe7d8` warm paper | `#1b1b1a` near-black | the page |
| `--on-ground` | `#141412` | `#dcc9a9` | type and rules on the page |
| `--box` | `#141412` | transparent | the 2px box rule |
| `--cream` | `#dcc9a9` | same | every content box |
| `--paper` | `#e8dcc6` | same | a figure's sheet; code inside a box |
| `--ink` | `#141412` | same | type on cream and paper |
| `--caption` | `#5a5246` | same | quiet type on cream: labels, column heads |
| `--green` / `--green-tint` | `#4e6851` / `#b9c7ad` | same | working, done well, new |
| `--ochre` / `--ochre-tint` | `#e0a35c` / `#efcf9f` | same | waiting, a cost, the owner's quoted ask |
| `--red` / `--red-tint` | `#b83a2d` / `#e9b4a9` | same | the owner is needed, or something is broken |
| `--grey` / `--grey-tint` | `#8f8a80` / 45% grey in paper | same | parked, muted (`.tag.grey`) |

The rules the tokens serve:

- **Boxes are cream with ink type, 16px corners.** In light every box wears a
  2px ink rule (`--box`), because paper and cream are too close to tell apart;
  in dark the ground does that and the rule goes.
- **Colour is signal.** A signal is a muted tint as the ground with ink on it
  and the full colour only as a 6px left edge (`.tone-green`, `.tone-ochre`,
  `.tone-red`), a tag or a mark. Never light type on a signal colour.
- **Red only for what needs the owner or is broken.** Never for emphasis.
- **Type.** JetBrains Mono, heavy and uppercase, for the masthead, section
  heads, labels and figures' names; IBM Plex Sans for prose. Mono runs with
  ligatures off so `-<` and `->` stay as written.
- **The masthead** is an `h1` of two to four words in big uppercase mono, and
  a `.dateline` in small tracked mono: repository, area, author, kind of
  page, date, the issue it sits under. A 4px rule closes it.
- **Section heads** are uppercase mono `h2`s under a 2px rule.
- **Figures** sit on a paper sheet in both themes: SVG boxes with 2px ink
  outlines, tinted fills by role, ink arrows, mono labels.
- **No emoji, no shadows.** Boxes are told apart by ground and rules.
- **Phone width works.** A 16px side gutter, grids that collapse to one
  column, and tables and figures that scroll inside their own box.

## The starter

`assets/page.html` is a content file: the masthead, the sections and one
figure as `Diagram` data, with no head and no stylesheet. Copy it, replace each
placeholder (every one says in prose what goes there), and assemble it into
the page that is kept and published:

```bash
python3 .agents/skills/architecture-diagram/assets/assemble.py content.html page.html --title "Two to four words"
```

Its parts:

| Class | Is |
|---|---|
| `header.mast`, `.dateline` | the masthead |
| `.lede` | the one paragraph under it |
| `.box`, `.box h3`, `.label` | a cream box, its heading, a tracked label |
| `.tone-green` / `-ochre` / `-red` | a box in a signal |
| `.stat` with `.fig` | a box led by one big figure |
| `.grid2`, `.grid3` | two or three boxes a row, one on a phone |
| `section.fig`, `.fig-no`, `.sheet`, `.key` | a figure: its ruled head, the paper sheet the `Diagram` call draws on, its key |
| `.table-wrap`, `.tag` | a table in a box; a verdict stamped in a tint (`.tag.green`, `.ochre`, `.red`, `.grey`) |
| `ol`, `ul` | numbered steps under a figure; a plain list in a box |

Every figure, the smallest flow included, is `Diagram` data drawn by
`/architecture-diagram`'s library, never hand-placed SVG.

## The order of a page

A page that asks the owner for something follows this order. A report keeps
the parts that apply, in the same order.

1. **The ask, in the owner's own words.** Quoted exactly, in an ochre box under
   the lede. A page with no owner's ask opens with the question it answers.
2. **Why now, with real numbers.** Two or three stat boxes, each a number read
   from the ledger, a log, a run or an issue, with where it was read.
3. **How it works.** The figure first, then numbered steps, one per box or
   arrow, in the figure's order.
4. **Inputs first.** Everything the mechanism reads, named field by field,
   before anything it produces.
5. **Worked examples.** Real cases from logs, issues or runs, each with its
   context and the outcome the design gives it.
6. **Where each piece lives.** Files, commands and types, split by the home
   each belongs to; a fleet piece is named in fleet's own nouns.
7. **Costs.** What it spends: runs, sessions, tokens, a person's time.
8. **Decisions for the owner.** One box per question: the proposal, the
   alternative, what each costs. The last box is the next step once ruled.

## Writing

- **Plain words.** Say what a person would: "waiting for a free session", not
  the state's code name. A step reads as what it does.
- **Real data, nothing invented.** Ids, shas, counts and dates come from where
  they were read, and are the page's texture. A placeholder never ships.
- **An unknown is said as unknown,** with what would tell it. A guess is not
  dressed as a figure, and a time estimate is not given.
- **Name what a reader can open:** the command, the file, the type, the issue.
- **Titles first, codes small.** A box's heading is what the thing is called;
  an id is the small print beside it.

## Render and look before publishing

A page is shown to nobody until its author has looked at it in both themes:

```bash
.agents/skills/architecture-diagram/assets/render.sh page.html <out-dir> [height] [width]
```

writes `light-N.png` and `dark-N.png` slices of a capture 1200 wide by
default; run it once more at `500` wide, the narrowest headless Chrome
draws, which already puts every grid in one column. Open every
slice. Look for text touching a box edge, a box rule missing in light, a
signal colour used as a text ground, red where nothing needs the owner, a
table or figure overflowing its box, a placeholder left in, and a heading that
wraps badly. At phone width, look for a page that scrolls sideways
and a grid that kept two columns. Fix and render again.

## Where it lives

A page is one file, kept wherever its author names and published from it;
the owner's changes are made in that file and published again.

## Publishing

A page is published as a claude.ai artifact, private until shared, from the
file it is kept in, and published again from the same file after each change
so its link stays the same. As of 2026-10-25 a self-hosted artifact site for
these pages is planned, and this section is the one to change when it ships.
