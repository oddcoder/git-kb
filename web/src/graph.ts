// The graph of pages and their links, moved by a small force simulation: pages push each other apart, links
// pull them together like springs, and everything is drawn gently toward the middle, so the graph settles into
// its own shape and then floats. Drag a page to move it and let it spring back; hover a page to light it and its
// neighbours; click it to open it. With reduced motion, the graph settles and stays still.
import { $$ } from './dom';

/** A page in the graph, as the server gives it: title, href, kind, and hops from the page shown. */
interface GraphNode { t: string; h: string; k: 'here' | 'page' | 'missing'; d: number }
/** A link: the two pages' indexes, and whether it goes both ways. */
type GraphEdge = [number, number, 1?];
interface Body extends GraphNode {
  x: number; y: number; vx: number; vy: number; phase: number; near: Set<Body>;
  r: number; g: SVGGElement; at: [number, number];
}
interface Line { s: Body; t: Body; both: boolean; path: SVGPathElement }

const NS = 'http://www.w3.org/2000/svg';
const still = matchMedia('(prefers-reduced-motion: reduce)').matches;

for (const box of $$('.kb-graph')) draw(box);
// A fresh copy of the page from live.js brings its own graph, to draw; the old one's motion stops with it.
document.addEventListener('kb-refresh', () => $$('.kb-graph:not(.drawn)').forEach(draw));

function el<K extends keyof SVGElementTagNameMap>(name: K, attrs: Record<string, string | number>, parent?: Element): SVGElementTagNameMap[K] {
  const e = document.createElementNS(NS, name);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
  parent?.append(e);
  return e;
}

function draw(box: HTMLElement): void {
  box.classList.add('drawn');
  const { nodes, edges } = JSON.parse(box.dataset.graph ?? '{"nodes":[],"edges":[]}') as { nodes: GraphNode[]; edges: GraphEdge[] };
  if (!nodes.length) return;
  const full = box.classList.contains('full');
  let [W, H] = [box.clientWidth, box.clientHeight];
  // Forces as d3-force has them: pages repel as 1/distance, links pull toward their length, and a weak pull
  // to the middle keeps loose pages near. Positions are the simulation's own; the view scales them to the box.
  const LENGTH = full ? 70 : 40, PUSH = 60, PULL = 0.3, CENTRE = 0.03, FRICTION = 0.6, PAD = 18;
  // Room kept on the right for the labels, which sit to the right of their pages.
  const LABEL_ROOM = full ? 90 : 96, ZOOM = full ? 3 : 2.5, SIZE = full ? 1.5 : 1;
  let LABEL = Math.min(LABEL_ROOM, W * 0.35);

  const svg = el('svg', { viewBox: `0 0 ${W} ${H}`, width: W, height: H });
  box.replaceChildren(svg);
  const head = `g-head-${Math.random().toString(36).slice(2)}`;
  const marker = el('marker', { id: head, viewBox: '0 0 6 6', refX: 5, refY: 3, markerWidth: 5, markerHeight: 5, orient: 'auto-start-reverse' },
    el('defs', {}, svg));
  el('path', { class: 'g-head', d: 'M0 0L6 3L0 6z' }, marker);
  const lines = el('g', { class: 'edges' }, svg);
  const dots = el('g', { class: 'nodes' }, svg);

  // Pages start on a spiral around the middle, the page itself at its centre.
  const ns: Body[] = nodes.map((n, i) => {
    const a = i * 2.4, r = n.k === 'here' ? 0 : 10 * Math.sqrt(i + 1);
    const size = SIZE * (n.k === 'here' ? 7 : n.k === 'missing' ? 3.5 : n.d > 1 ? 3.5 : 5);
    const g = el('g', { class: `g-node ${n.k} hop${n.d}`, tabindex: n.h ? 0 : -1 }, dots);
    return { ...n, x: Math.cos(a) * r, y: Math.sin(a) * r, vx: 0, vy: 0, phase: i * 1.7, near: new Set<Body>(), r: size, g, at: [0, 0] };
  });
  const es: Line[] = edges.map(([s, t, both]) => {
    const path = el('path', { class: 'g-edge', 'marker-end': `url(#${head})` }, lines);
    if (both) path.setAttribute('marker-start', `url(#${head})`);
    return { s: ns[s], t: ns[t], both: !!both, path };
  });
  for (const e of es) { e.s.near.add(e.t); e.t.near.add(e.s); }

  for (const n of ns) {
    el('circle', { r: n.r }, n.g);
    // Beside the page, short labels; the whole title is in its tooltip.
    const label = n.k === 'missing' && !full ? n.t.replace(/ \(missing\)$/, '') : n.t;
    el('text', { x: n.r + 4, y: 3.5 }, n.g).textContent = !full && label.length > 20 ? `${label.slice(0, 19)}…` : label;
    el('title', {}, n.g).textContent = n.t;
  }

  // Hovering lights a page, its links and its neighbours, and dims the rest.
  const light = (n: Body | null) => {
    box.classList.toggle('focus', !!n);
    for (const m of ns) m.g.classList.toggle('lit', !!n && (m === n || n.near.has(m)));
    for (const e of es) e.path.classList.toggle('lit', !!n && (e.s === n || e.t === n));
  };

  let heat = 1, time = 0;
  let drag: { n: Body; moved: boolean; x0: number; y0: number } | null = null;
  // The view: the simulation's point at the box's centre, and its scale, eased toward fitting every page in.
  const view = { x: 0, y: 0, s: 1 };
  const fit = (ease: number) => {
    if (drag) return;
    const xs = ns.map((n) => n.x), ys = ns.map((n) => n.y);
    const [x0, x1, y0, y1] = [Math.min(...xs), Math.max(...xs), Math.min(...ys), Math.max(...ys)];
    const s = Math.min(ZOOM, (W - 2 * PAD - LABEL) / Math.max(x1 - x0, 1), (H - 2 * PAD) / Math.max(y1 - y0, 1));
    view.x += ((x0 + x1) / 2 - view.x) * ease;
    view.y += ((y0 + y1) / 2 - view.y) * ease;
    view.s += (s - view.s) * ease;
  };
  let middle = (W - LABEL) / 2;
  const screen = (x: number, y: number): [number, number] => [(x - view.x) * view.s + middle, (y - view.y) * view.s + H / 2];
  const point = (ev: PointerEvent): [number, number] => {
    const b = svg.getBoundingClientRect();
    return [(ev.clientX - b.left - middle) / view.s + view.x, (ev.clientY - b.top - H / 2) / view.s + view.y];
  };
  for (const n of ns) {
    n.g.addEventListener('pointerenter', () => light(n));
    n.g.addEventListener('pointerleave', () => drag || light(null));
    n.g.addEventListener('pointerdown', (ev) => {
      drag = { n, moved: false, x0: ev.clientX, y0: ev.clientY };
      svg.setPointerCapture(ev.pointerId);
      ev.preventDefault();
    });
    n.g.addEventListener('keydown', (ev) => { if (ev.key === 'Enter' && n.h) location.href = n.h; });
  }
  svg.addEventListener('pointermove', (ev) => {
    if (!drag) return;
    // A click that wobbles a pixel or two is still a click.
    drag.moved ||= Math.hypot(ev.clientX - drag.x0, ev.clientY - drag.y0) > 4;
    if (!drag.moved) return;
    [drag.n.x, drag.n.y] = point(ev);
    drag.n.vx = drag.n.vy = 0;
    heat = Math.max(heat, 0.4);
  });
  svg.addEventListener('pointerup', () => {
    if (drag && !drag.moved && drag.n.h) location.href = drag.n.h;
    drag = null;
    light(null);
  });

  function step(): void {
    const k = heat;
    for (let i = 0; i < ns.length; i++) {
      for (let j = i + 1; j < ns.length; j++) {
        const a = ns[i], b = ns[j];
        const dx = a.x - b.x, dy = a.y - b.y, d2 = dx * dx + dy * dy + 1;
        const f = (PUSH * k) / d2;
        a.vx += dx * f; a.vy += dy * f; b.vx -= dx * f; b.vy -= dy * f;
      }
    }
    for (const { s, t } of es) {
      const dx = t.x - s.x, dy = t.y - s.y, d = Math.hypot(dx, dy) || 1;
      const f = ((d - LENGTH) / d) * PULL * k / 2;
      s.vx += dx * f; s.vy += dy * f; t.vx -= dx * f; t.vy -= dy * f;
    }
    for (const n of ns) {
      if (drag?.n === n) continue;
      n.vx -= n.x * CENTRE * k;
      n.vy -= n.y * CENTRE * k;
      n.vx *= FRICTION; n.vy *= FRICTION;
      n.x += n.vx;
      n.y += n.vy;
    }
    heat = Math.max(heat * 0.985, still ? 0 : 0.03);
  }

  // Each page floats a little around where the forces put it, each on its own rhythm.
  function render(): void {
    fit(0.08);
    const at = (n: Body): [number, number] => {
      const [x, y] = screen(n.x, n.y);
      return still ? [x, y] : [x + Math.sin(time * 0.0011 + n.phase) * 1.6, y + Math.cos(time * 0.0009 + n.phase) * 1.6];
    };
    for (const n of ns) { n.at = at(n); n.g.setAttribute('transform', `translate(${n.at[0]},${n.at[1]})`); }
    // Links bow slightly, all the same way round, and stop at the edge of the page they point to.
    for (const e of es) {
      const [x1, y1] = e.s.at, [x2, y2] = e.t.at, dx = x2 - x1, dy = y2 - y1;
      const [cx, cy] = [(x1 + x2) / 2 - dy * 0.12, (y1 + y2) / 2 + dx * 0.12];
      const [ux, uy] = [(x2 - cx), (y2 - cy)], u = Math.hypot(ux, uy) || 1, cut = e.t.r + 2;
      // A two-way line stops short of both pages, so both its heads show.
      const [vx, vy] = [(cx - x1), (cy - y1)], v = Math.hypot(vx, vy) || 1, back = e.both ? e.s.r + 2 : 0;
      e.path.setAttribute('d', `M${x1 + (vx / v) * back},${y1 + (vy / v) * back}Q${cx},${cy} ${x2 - (ux / u) * cut},${y2 - (uy / u) * cut}`);
    }
  }

  // Settle before the first frame, so the graph appears in shape rather than unfolding.
  for (let i = 0; i < 300; i++) step();
  fit(1);
  // When the box changes size, as when the sidebar is dragged wider, the view fits the graph to it again.
  new ResizeObserver(() => {
    [W, H] = [box.clientWidth, box.clientHeight];
    LABEL = Math.min(LABEL_ROOM, W * 0.35);
    middle = (W - LABEL) / 2;
    Object.entries({ viewBox: `0 0 ${W} ${H}`, width: W, height: H }).forEach(([k, v]) => svg.setAttribute(k, String(v)));
    render();
  }).observe(box);
  let visible = true;
  new IntersectionObserver(([e]) => { visible = e.isIntersecting; }).observe(box);
  const frame = (t: number) => {
    if (!box.isConnected) return;
    time = t;
    if (visible && !document.hidden && (heat > 0.031 || !still || drag)) { step(); render(); }
    requestAnimationFrame(frame);
  };
  render();
  requestAnimationFrame(frame);
}
