// Draws Plonk Press figures from data: Diagram.arch for a structure, Diagram.seq for a sequence.
// Each call fills an empty <svg class="d" id="..."> already on the page.
(function () {
  const NS = "http://www.w3.org/2000/svg";
  const KINDS = ["int", "ctl", "tool", "db", "wire"];
  // The fill a text sits on, so its halo erases the lines passing under it.
  const BG = {
    cluster: "--paper", svc: "--paper", ext: "--paper", pill: "--paper",
    "ns:plat": "--plat", "ns:user": "--user", db: "--user", chip: "--cream",
    proc: "--new-tint", "svc:new": "--new-tint", "chip:new": "--new-tint",
  };

  const theme = new URLSearchParams(location.search).get("theme");
  if (theme === "light" || theme === "dark") document.documentElement.setAttribute("data-theme", theme);

  function el(tag, attrs, parent) {
    const node = document.createElementNS(NS, tag);
    for (const k in attrs) if (attrs[k] !== undefined) node.setAttribute(k, attrs[k]);
    if (parent) parent.appendChild(node);
    return node;
  }

  function text(parent, x, y, spans, opts = {}) {
    const t = el("text", { x, y, "text-anchor": opts.anchor, transform: opts.rotate ? `rotate(${opts.rotate} ${x} ${y})` : undefined }, parent);
    if (opts.halo) { t.classList.add("halo"); t.style.stroke = `var(${opts.halo})`; }
    (Array.isArray(spans) ? spans : [[spans, opts.cls || ""]]).forEach(([s, cls], i) => {
      const ts = el("tspan", { class: cls, dx: i ? 10 : undefined }, t);
      ts.textContent = s;
    });
    return t;
  }

  function markers(svg, id) {
    const defs = el("defs", {}, svg);
    for (const k of KINDS) {
      const m = el("marker", { id: `${id}-${k}`, viewBox: "0 0 10 10", refX: 9, refY: 5, markerWidth: 12, markerHeight: 12, markerUnits: "userSpaceOnUse", orient: "auto-start-reverse" }, defs);
      el("path", { d: "M0,0 L10,5 L0,10 z", class: `f-${k}` }, m);
    }
    return defs;
  }

  function title(svg, words) {
    if (!words) return;
    el("title", {}, svg).textContent = words;
    svg.setAttribute("role", "img");
  }

  const isNew = b => b.kind === "proc" || ((b.kind === "svc" || b.kind === "chip") && b.isNew);

  function bgKey(b) {
    if (b.kind === "ns") return `ns:${b.tone || "plat"}`;
    if (isNew(b) && b.kind !== "proc") return `${b.kind}:new`;
    return b.kind;
  }

  function bgAt(boxes, x, y) {
    let bg = "--paper";
    for (const b of boxes) if (x >= b.x && x <= b.x + b.w && y >= b.y && y <= b.y + b.h && BG[bgKey(b)]) bg = BG[bgKey(b)];
    return bg;
  }

  const SHAPE = {
    cluster: () => ({ class: "cluster-box", rx: 16 }),
    ns: b => ({ class: `ns-box ${b.tone || "plat"}`, rx: 12 }),
    pod: () => ({ class: "pod-box", rx: 10 }),
    svc: () => ({ class: "svc-box", rx: 10 }),
    db: () => ({ class: "db-box", rx: 8 }),
    chip: () => ({ class: "chip", rx: 6 }),
    ext: () => ({ class: "ext-box", rx: 10 }),
    pill: b => ({ class: "pill", rx: b.h / 2 }),
  };
  const RADIUS = { proc: 10, svc: 10, chip: 6 };

  // A new piece: green tint, a full-green band down its left edge, the ink outline over both.
  function newBox(g, defs, b, key, edge = 6) {
    const rx = RADIUS[b.kind] ?? 10;
    const clip = el("clipPath", { id: key }, defs);
    el("rect", { x: b.x, y: b.y, width: b.w, height: b.h, rx }, clip);
    el("rect", { x: b.x, y: b.y, width: b.w, height: b.h, rx, class: "new-fill" }, g);
    el("rect", { x: b.x, y: b.y, width: edge, height: b.h, class: "new-edge", "clip-path": `url(#${key})` }, g);
    el("rect", { x: b.x, y: b.y, width: b.w, height: b.h, rx, class: "outline" }, g);
  }

  function boxText(g, b, boxes) {
    const halo = BG[bgKey(b)];
    const x = b.x, y = b.y, w = b.w, h = b.h;
    const pad = isNew(b) ? 20 : 16;
    switch (b.kind) {
      case "cluster":
        text(g, x + 24, y + 36, b.name, { cls: "t-lg", halo });
        if (b.note) text(g, x + w - 24, y + 34, b.note, { cls: "t-xs t-soft", anchor: "end", halo });
        break;
      case "ns":
        text(g, x + 16, y + 26, [[b.tag || "NAMESPACE", "t-kind"], [b.name, "t-sm"]], { halo });
        break;
      case "pod":
        text(g, x + 16, y + 24, [[b.tag || "POD", "t-kind"], [b.name, "t-sm"]], { halo: bgAt(boxes.filter(o => o !== b), x + 16, y + 24) });
        break;
      case "proc":
        text(g, x + pad, y + 24, [[b.tag || "PROCESS", "t-kind"], [b.name, "t-sm"], ["NEW", "t-kind t-new"]], { halo });
        break;
      case "svc":
        if (b.sub) {
          text(g, x + pad, y + 28, b.name, { cls: "t-md", halo });
          text(g, x + pad, y + 50, b.sub, { cls: "t-sans t-sm t-soft", halo });
        } else {
          text(g, x + pad, y + h / 2 + 6, b.name, { cls: "t-md", halo });
        }
        if (b.isNew) text(g, x + w - 14, y + 24, "NEW", { cls: "t-kind t-new", anchor: "end", halo });
        break;
      case "db":
      case "ext":
        text(g, x + 14, y + 22, b.tag || (b.kind === "db" ? "DATABASE" : "EXTERNAL"), { cls: "t-kind", halo });
        text(g, x + 14, y + 46, b.name, { cls: "t-md", halo });
        if (b.sub) text(g, x + 14, y + 64, b.sub, { cls: "t-sans t-xs t-soft", halo });
        if (b.note) text(g, x + w - 14, y + 22, b.note, { cls: "t-kind", anchor: "end", halo });
        if (b.caption) text(g, x, y + h + 22, b.caption, { cls: "t-xs t-soft" });
        break;
      case "chip":
        text(g, x + w / 2 + (b.isNew ? 3 : 0), y + h / 2 + 4, b.name, { cls: "t-xs", anchor: "middle", halo });
        break;
      case "pill":
        text(g, x + w / 2, y + h / 2 + 5, b.name, { cls: "t-sm", anchor: "middle", halo });
        break;
    }
  }

  // spec: { width, height, title, boxes: [...], lines: [...], labels: [...] }
  // Boxes are listed outermost first; lines are drawn over boxes and under every text.
  function arch(id, spec) {
    const svg = document.getElementById(id);
    svg.setAttribute("viewBox", `0 0 ${spec.width || 1190} ${spec.height}`);
    title(svg, spec.title);
    const defs = markers(svg, id);
    const boxes = spec.boxes || [];
    const gBox = el("g", {}, svg), gLine = el("g", {}, svg), gText = el("g", {}, svg);
    boxes.forEach((b, i) => {
      if (isNew(b)) newBox(gBox, defs, b, `${id}-new-${i}`);
      else el("rect", { x: b.x, y: b.y, width: b.w, height: b.h, ...SHAPE[b.kind](b) }, gBox);
    });
    for (const l of spec.lines || []) {
      const k = l.k || "int";
      el("path", {
        d: l.d, class: `ln-${k}${(l.dash && k !== "ctl") || k === "wire" ? " dash" : ""}`,
        "marker-end": l.end === false ? undefined : `url(#${id}-${k})`,
        "marker-start": l.both ? `url(#${id}-${k})` : undefined,
      }, gLine);
      if (l.label) {
        const [lx, ly] = l.at;
        text(gText, lx, ly, l.label, { cls: `t-line${k === "tool" ? " t-new" : ""}`, anchor: l.anchor, rotate: l.rotate, halo: bgAt(boxes, lx, ly) });
      }
    }
    for (const b of boxes) boxText(gText, b, boxes);
    for (const t of spec.labels || []) text(gText, t.x, t.y, t.text, { cls: t.cls || "t-sans t-sm t-soft", anchor: t.anchor, halo: bgAt(boxes, t.x, t.y) });
  }

  // parts: [{ id, tag, label, kind: "user" | "new" | "store" | "svc" }]
  // steps: [{ from, to, text, k, reply }] or [{ note: [fromId, toId], text }]
  function seq(id, parts, steps, opts = {}) {
    const svg = document.getElementById(id);
    title(svg, opts.title);
    const boxW = parts.length > 6 ? 112 : 140, headH = 58, top = 12, rowH = 40, left = 24;
    const colW = Math.min(168, (980 - left * 2 - boxW) / (parts.length - 1));
    const cx = i => left + boxW / 2 + i * colW;
    const width = left * 2 + boxW + colW * (parts.length - 1);
    const firstY = top + headH + 36;
    const height = firstY + steps.length * rowH + 10;
    svg.setAttribute("viewBox", `0 0 ${width} ${height}`);
    const defs = markers(svg, id);
    const idx = {};
    parts.forEach((p, i) => (idx[p.id] = i));
    const gLine = el("g", {}, svg), gText = el("g", {}, svg);

    parts.forEach((p, i) => {
      el("line", { x1: cx(i), y1: top + headH, x2: cx(i), y2: height - 6, class: "lifeline" }, gLine);
      const box = { kind: "svc", x: cx(i) - boxW / 2, y: top, w: boxW, h: headH };
      if (p.kind === "new") newBox(gLine, defs, box, `${id}-head-${i}`);
      else {
        const cls = p.kind === "user" ? "pill" : p.kind === "store" ? "db-box" : "svc-box";
        el("rect", { x: box.x, y: top, width: boxW, height: headH, rx: p.kind === "user" ? 29 : 10, class: cls }, gLine);
      }
      const nx = cx(i) + (p.kind === "new" ? 3 : 0);
      text(gText, nx, top + 23, p.tag, { cls: "t-kind", anchor: "middle" });
      text(gText, nx, top + 43, p.label, { cls: "t-sm", anchor: "middle" }).style.fontWeight = 800;
    });

    let n = 0;
    steps.forEach((s, r) => {
      const y = firstY + r * rowH;
      if (s.note) {
        const a = idx[s.note[0]], b = idx[s.note[1]];
        const x1 = cx(Math.min(a, b)) - boxW / 2 + 6, x2 = cx(Math.max(a, b)) + boxW / 2 - 6;
        const note = { kind: "chip", x: x1, y: y - 14, w: x2 - x1, h: 26 };
        const clip = el("clipPath", { id: `${id}-note-${r}` }, defs);
        el("rect", { x: note.x, y: note.y, width: note.w, height: note.h, rx: 5 }, clip);
        el("rect", { x: note.x, y: note.y, width: note.w, height: note.h, rx: 5, class: "note-box" }, gLine);
        el("rect", { x: note.x, y: note.y, width: 5, height: note.h, class: "note-edge", "clip-path": `url(#${id}-note-${r})` }, gLine);
        text(gText, (x1 + x2) / 2 + 2, y + 4, s.text, { cls: "t-sans t-sm", anchor: "middle" });
        return;
      }
      n += 1;
      const k = s.k || "int";
      const a = idx[s.from], b = idx[s.to];
      const lastCol = a === parts.length - 1;
      const cls = `ln-${k}${s.reply && k !== "ctl" ? " dash" : ""}`;
      const marker = `url(#${id}-${k})`;
      if (a === b) {
        const x = cx(a), dir = lastCol ? -1 : 1;
        el("path", { d: `M${x},${y - 6} H${x + 34 * dir} V${y + 10} H${x + 4 * dir}`, class: cls, "marker-end": marker }, gLine);
        text(gText, x + 42 * dir, y + 6, s.text, { cls: "t-sans t-sm", anchor: lastCol ? "end" : "start", halo: "--paper" });
      } else {
        el("path", { d: `M${cx(a) + (b > a ? 4 : -4)},${y} H${cx(b) + (b > a ? -4 : 4)}`, class: cls, "marker-end": marker }, gLine);
        text(gText, (cx(a) + cx(b)) / 2, y - 8, s.text, { cls: "t-sans t-sm", anchor: "middle", halo: "--paper" });
      }
      const numX = a === b ? cx(a) + (lastCol ? 16 : -16) : b > a ? cx(a) - 16 : cx(a) + 16;
      el("circle", { cx: numX, cy: y, r: 9, class: "num" }, gText);
      text(gText, numX, y + 3.5, String(n), { cls: "t-num", anchor: "middle" });
    });
  }

  window.Diagram = { arch, seq };
})();
