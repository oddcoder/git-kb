// The column of comments beside a served page: each card level with what it is on, pushed down as far as the card
// above it needs. The page and the column scroll each on their own, and keep in step: whichever the reader scrolls
// leads, and the other glides after it, so that a card stays level with its text. Hovering a card lights what it is
// on, with a line to it, and the other way round. Without this script the cards simply follow one another, and the
// column scrolls alone.
import { $, $$ } from './dom';

const root = document.documentElement;
// The parts of the page these work on, found again when live.ts swaps in a fresh copy of the page.
let cards = $('.rail-cards');
const column = () => cards?.closest<HTMLElement>('.rail') ?? null;
const all = (): HTMLElement[] => [...(cards?.children ?? [])].filter((c): c is HTMLElement => c instanceof HTMLElement);
const linesOf = (blk: HTMLElement) => (blk.dataset.lines ?? '0-0').split('-').map(Number);
// The block a line is in, by the lines each block holds.
const blockAt = (line: number) => $$('.blk[data-lines]').find((b) => { const [a, z] = linesOf(b); return a <= line && line < z; });
const layoutTop = () => $('.layout')?.getBoundingClientRect().top ?? 0;
const shown = (e: HTMLElement) => !e.hidden && getComputedStyle(e).display !== 'none';

// A place in the page, from the layout's top, and the place in the column, from the cards' top, of the same card.
interface Pair { page: number; col: number }
let pairs: Pair[] = [];
const GAP = 10;

// Each card where it wants to be: level with its block, in the page's own coordinates, so that its place does not
// depend on how either is scrolled; or, when that is taken, right under the card above it.
function place(): void {
  if (!cards) return;
  root.classList.add('rail-placed');
  if (!all().some((c) => getComputedStyle(c).position === 'absolute')) return;
  const top = layoutTop();
  const want = (card: HTMLElement) => (blockAt(Number(card.dataset.line))?.getBoundingClientRect().top ?? top) - top;
  const placed = all().filter(shown).map((c) => ({ c, y: want(c) })).sort((a, b) => a.y - b.y);
  // The column's head is above its cards, so that a card is level with its text with both scrolled to the top.
  const head = cards.offsetTop;
  let bottom = 0;
  pairs = [];
  for (const { c, y: wanted } of placed) {
    const y = Math.max(wanted - head, bottom);
    c.style.top = `${y}px`;
    bottom = y + c.offsetHeight + GAP;
    if (!pairs.length || wanted > pairs[pairs.length - 1].page) pairs.push({ page: wanted, col: y });
  }
  // Room below the last card, for the column to scroll as far as the page does.
  cards.style.height = `${Math.max(bottom, root.scrollHeight) + innerHeight}px`;
  match();
}

// A place in one, as the place in the other: between two cards, in proportion; above the first and below the last,
// one to one.
function across(v: number, from: keyof Pair, to: keyof Pair): number {
  if (!pairs.length) return v;
  const [first, last] = [pairs[0], pairs[pairs.length - 1]];
  if (v <= first[from]) return first[to] - (first[from] - v);
  if (v >= last[from]) return last[to] + (v - last[from]);
  const i = pairs.findIndex((p) => p[from] > v);
  const [p, q] = [pairs[i - 1], pairs[i]];
  return p[to] + ((v - p[from]) * (q[to] - p[to])) / Math.max(1, q[from] - p[from]);
}

// The two are kept in step at a reading line, a third of the way down the screen: the page's line, from the layout's
// top, and the column's, from the cards' top. The column scrolls only where it is beside the page, not under it.
const READING = 0.35;
const scrolled = () => { const box = column(); return box && getComputedStyle(box).overflowY === 'auto' ? box : null; };
const pageLine = () => innerHeight * READING - layoutTop();
const pageAt = (line: number) => scrollY + layoutTop() + line - innerHeight * READING;
const cardsTop = (box: HTMLElement) => box.getBoundingClientRect().top + (cards?.offsetTop ?? 0) - box.scrollTop;
const columnLine = (box: HTMLElement) => innerHeight * READING - cardsTop(box);
const columnAt = (box: HTMLElement, line: number) => box.scrollTop + cardsTop(box) + line - innerHeight * READING;

// Whichever the reader scrolls leads; the other eases toward where it should be, rather than jumping.
let leader: 'page' | 'column' = 'page', ticking = false;
const goal: { page: number | null; column: number | null } = { page: null, column: null };
function follow(): void {
  const box = scrolled();
  let more = false;
  if (box && goal.column != null) {
    const d = goal.column - box.scrollTop;
    if (Math.abs(d) < 0.5) { box.scrollTop = goal.column; goal.column = null; } else { box.scrollTop += d * 0.16; more = true; }
  }
  if (goal.page != null) {
    const d = goal.page - scrollY;
    if (Math.abs(d) < 0.5) { scrollTo(0, goal.page); goal.page = null; } else { scrollTo(0, scrollY + d * 0.16); more = true; }
  }
  connect();
  ticking = more;
  if (more) requestAnimationFrame(follow);
}
const glide = () => { if (!ticking) { ticking = true; requestAnimationFrame(follow); } };
// The column brought to the page at once, as when the cards are placed again.
function match(): void {
  const box = scrolled();
  if (box) box.scrollTop = columnAt(box, across(pageLine(), 'page', 'col'));
}
// The leader is whichever the reader's wheel, touch, pointer or keys are on.
for (const type of ['wheel', 'touchstart', 'pointerdown', 'keydown'] as const) {
  document.addEventListener(type, (e: Event) => {
    const on = type === 'keydown' ? document.activeElement : e.target instanceof Node ? e.target : null;
    const now = on && column()?.contains(on) ? 'column' : 'page';
    if (now !== leader) { leader = now; goal.page = goal.column = null; }
  }, { capture: true, passive: true });
}
addEventListener('scroll', () => {
  const box = scrolled();
  if (box && leader === 'page') { goal.column = columnAt(box, across(pageLine(), 'page', 'col')); glide(); }
}, { passive: true });
document.addEventListener('scroll', (e) => {
  const box = scrolled();
  if (box && e.target === box && leader === 'column') { goal.page = pageAt(across(columnLine(box), 'col', 'page')); glide(); }
}, { capture: true, passive: true });

// A card and what it is on, lit together while either is hovered, with a line from the card to its text, drawn over
// the page and kept up as either scrolls: from the card's left edge to its block's right, at the height nearest the
// card that is on the screen, so that a tall block's line does not run off to its top.
const SVG = 'http://www.w3.org/2000/svg';
const lines = document.createElementNS(SVG, 'svg');
lines.classList.add('cmt-lines');
document.body.append(lines);
const linked = new Set<HTMLElement>();
function connect(): void {
  lines.replaceChildren();
  for (const card of linked) {
    const to = blockAt(Number(card.dataset.line))?.getBoundingClientRect();
    if (!to || !card.isConnected || !shown(card)) continue;
    const from = card.getBoundingClientRect();
    const [x1, y1] = [from.left, from.top + 14];
    const [x2, y2] = [to.right + 6, Math.min(Math.max(y1, to.top + 12, 64), to.bottom - 12, innerHeight - 12)];
    const bend = Math.max(40, (x1 - x2) / 2);
    const path = document.createElementNS(SVG, 'path');
    path.setAttribute('d', `M${x1},${y1} C${x1 - bend},${y1} ${x2 + bend},${y2} ${x2},${y2}`);
    const dot = document.createElementNS(SVG, 'circle');
    Object.entries({ cx: x2, cy: y2, r: 3 }).forEach(([k, v]) => dot.setAttribute(k, String(v)));
    lines.append(path, dot);
  }
}
function lit(card: HTMLElement, on: boolean): void {
  card.classList.toggle('lit', on);
  blockAt(Number(card.dataset.line))?.classList.toggle('cmt-lit', on);
  if (on) linked.add(card); else linked.delete(card);
  connect();
}
addEventListener('scroll', connect, { passive: true });
document.addEventListener('scroll', connect, { capture: true, passive: true });

// Cards placed again whenever any grows or shrinks, as a reply box opens or a thread folds, and when the page does.
let pending = false;
const again = () => { if (!pending) { pending = true; requestAnimationFrame(() => { pending = false; place(); }); } };
const sizes = new ResizeObserver(again);
function hook(): void {
  cards = $('.rail-cards');
  sizes.disconnect();
  const page = $('main.page');
  if (page) sizes.observe(page);
  for (const card of all()) sizes.observe(card);
  linked.clear();
  connect();
  for (const card of all()) {
    card.addEventListener('mouseenter', () => lit(card, true));
    card.addEventListener('mouseleave', () => lit(card, false));
  }
  for (const blk of $$('.blk[data-lines]')) {
    const [a, z] = linesOf(blk);
    const mine = all().filter((c) => { const line = Number(c.dataset.line); return a <= line && line < z; });
    if (!mine.length) continue;
    blk.addEventListener('mouseenter', () => mine.forEach((c) => lit(c, true)));
    blk.addEventListener('mouseleave', () => mine.forEach((c) => lit(c, false)));
  }
  $('#show-resolved')?.addEventListener('change', again);
  place();
}
document.addEventListener('DOMContentLoaded', hook);
document.addEventListener('kb-refresh', hook);
addEventListener('resize', again);
