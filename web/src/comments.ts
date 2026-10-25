// The column of comments beside a served page: each card level with what it is on, pushed down as far as the card
// above it needs. The page and the column scroll each on their own, and keep in step: whichever the reader scrolls
// leads, and the other glides after it, so that a card stays level with its text. Hovering a card lights what it is
// on, with a line to it, and the other way round. Without this script the cards simply follow one another, and the
// column scrolls alone. What each card is on is lit in the text: the words it is on, or else its paragraph; clicking
// them brings the card to them.
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

// The text of blocks as one string, with each run of white space one space, and, for each of its characters, the
// text node and offset it came from, so that a match in the string can be turned back into a range of the page.
// What is beside the text, as its markers and the cards of its changes, is not its text.
type TextMap = [Text, number][];
function text(region: HTMLElement[]): { s: string; map: TextMap } {
  let s = '', space = true;
  const map: TextMap = [];
  for (const blk of region) {
    const walk = document.createTreeWalker(blk, NodeFilter.SHOW_TEXT, {
      acceptNode: (n) => (n.parentElement?.closest('.pop, .blame, .cmts, script, style, .kb-stage')
        ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT),
    });
    for (let node = walk.nextNode() as Text | null; node; node = walk.nextNode() as Text | null) {
      for (let i = 0; i < node.data.length; i++) {
        const white = /\s/.test(node.data[i]);
        if (white && space) continue;
        s += white ? ' ' : node.data[i];
        map.push([node, i]);
        space = white;
      }
    }
    if (!space && map.length) { s += ' '; map.push(map[map.length - 1]); space = true; }
  }
  return { s, map };
}
const squeeze = (t: string) => t.replace(/\s+/g, ' ').trim();

// The share of two lists of words they have in common, as the server measures it.
function likeness(a: string[], b: string[]): number {
  if (!a.length && !b.length) return 1;
  const row = new Array<number>(b.length + 1).fill(0);
  for (let i = 1; i <= a.length; i++) {
    let prev = 0;
    for (let j = 1; j <= b.length; j++) {
      const keep = row[j];
      row[j] = a[i - 1] === b[j - 1] ? prev + 1 : Math.max(row[j], row[j - 1]);
      prev = keep;
    }
  }
  return (2 * row[b.length]) / (a.length + b.length);
}

// Where a card's words are in a region's text: exactly, the occurrence with the most of the words around them; or
// else the run of words most like them, if like enough. As [start, end) in the string, or null.
function find(s: string, { exact = '', prefix = '', suffix = '' }: DOMStringMap): [number, number] | null {
  const want = squeeze(exact);
  if (!want) return null;
  const hits: number[] = [];
  for (let i = s.indexOf(want); i >= 0; i = s.indexOf(want, i + 1)) hits.push(i);
  const around = (i: number, len: number) =>
    likeness(squeeze(prefix).split(' '), s.slice(Math.max(0, i - prefix.length), i).trim().split(' '))
    + likeness(squeeze(suffix).split(' '), s.slice(i + len, i + len + suffix.length).trim().split(' '));
  if (hits.length) {
    const best = hits.reduce((a, b) => (around(b, want.length) > around(a, want.length) ? b : a));
    return [best, best + want.length];
  }
  const words = [...s.matchAll(/\S+/g)], wanted = want.split(' ');
  let best: { score: number; start: number; end: number } | null = null;
  for (let i = 0; i < words.length; i++) {
    for (let n = Math.max(1, wanted.length - 3); n <= wanted.length + 3 && i + n <= words.length; n++) {
      const run = words.slice(i, i + n);
      const score = likeness(wanted, run.map((w) => w[0]));
      if (score >= 0.75 && (!best || score > best.score)) {
        best = { score, start: run[0].index ?? 0, end: (run[n - 1].index ?? 0) + run[n - 1][0].length };
      }
    }
  }
  return best && [best.start, best.end];
}

// What each card is on, as a range of the page: its words, or else all of its paragraph's text. The cards shown are
// lit in the text, and the one hovered more.
const on = new Map<HTMLElement, Range>();
const lights = () => typeof CSS !== 'undefined' && 'highlights' in CSS;
function light(): void {
  on.clear();
  for (const card of all()) {
    if (!card.dataset.lines) continue;
    const [first, end] = linesOf(card);
    const region = $$('.blk[data-lines]').filter((b) => { const [a, z] = linesOf(b); return a < end && first < z; });
    const { s, map } = text(region);
    if (!map.length) continue;
    const [start, stop] = find(s, card.dataset) ?? [0, s.trimEnd().length];
    if (stop <= start) continue;
    const range = document.createRange();
    range.setStart(...map[start]);
    const [node, offset] = map[stop - 1];
    range.setEnd(node, offset + 1);
    on.set(card, range);
  }
  if (!lights()) return;
  CSS.highlights.set('kb-commented', new Highlight(...[...on].filter(([c]) => shown(c)).map(([, r]) => r)));
  CSS.highlights.set('kb-commented-lit', new Highlight());
}
// Where a card's words are on the screen, if they can be seen; words in a folded diagram's source cannot.
function wordsOf(card: HTMLElement): DOMRect | undefined {
  const range = card.dataset.exact ? on.get(card) : undefined;
  if (!range || range.startContainer.parentElement?.checkVisibility?.() === false) return undefined;
  const box = range.getBoundingClientRect();
  return box.height ? box : undefined;
}
// Where a card is on the screen: its words, or else its paragraph.
const spotOf = (card: HTMLElement) => wordsOf(card) ?? blockAt(Number(card.dataset.line))?.getBoundingClientRect();

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
  const want = (card: HTMLElement) => (spotOf(card)?.top ?? top) - top;
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
// the page and kept up as either scrolls: from the card's left edge to the end of its words, or else to its block's
// right, at the height nearest the card that is on the screen, so that a tall block's line does not run off to its
// top.
const SVG = 'http://www.w3.org/2000/svg';
const lines = document.createElementNS(SVG, 'svg');
lines.classList.add('cmt-lines');
document.body.append(lines);
const linked = new Set<HTMLElement>();
function connect(): void {
  lines.replaceChildren();
  for (const card of linked) {
    const seen = wordsOf(card) && on.get(card)?.getClientRects();
    const words = seen?.length ? seen[seen.length - 1] : null;
    const to = words ?? blockAt(Number(card.dataset.line))?.getBoundingClientRect();
    if (!to || !card.isConnected || !shown(card)) continue;
    const from = card.getBoundingClientRect();
    const [x1, y1] = [from.left, from.top + 14];
    const [x2, y2] = words ? [to.right, to.top + Math.min(to.height / 2, 12)]
      : [to.right + 6, Math.min(Math.max(y1, to.top + 12, 64), to.bottom - 12, innerHeight - 12)];
    const bend = Math.max(40, (x1 - x2) / 2);
    const path = document.createElementNS(SVG, 'path');
    path.setAttribute('d', `M${x1},${y1} C${x1 - bend},${y1} ${x2 + bend},${y2} ${x2},${y2}`);
    const dot = document.createElementNS(SVG, 'circle');
    Object.entries({ cx: x2, cy: y2, r: 3 }).forEach(([k, v]) => dot.setAttribute(k, String(v)));
    lines.append(path, dot);
  }
}
const onOf = (card: HTMLElement) => { const r = on.get(card); return r ? [r] : []; };
function lit(card: HTMLElement, now: boolean): void {
  card.classList.toggle('lit', now);
  blockAt(Number(card.dataset.line))?.classList.toggle('cmt-lit', now);
  if (now) linked.add(card); else linked.delete(card);
  if (lights()) CSS.highlights.set('kb-commented-lit', new Highlight(...[...linked].flatMap(onOf)));
  connect();
}

// Clicking words lit for a card brings the card to them: the page glides to put them on the reading line, and the
// column, following, puts the card there too; the card flashes.
function caret(x: number, y: number): [Node, number] | null {
  const at = document.caretPositionFromPoint?.(x, y);
  if (at) return [at.offsetNode, at.offset];
  const r = document.caretRangeFromPoint?.(x, y);
  return r ? [r.startContainer, r.startOffset] : null;
}
document.addEventListener('click', (e) => {
  if (!(getSelection()?.isCollapsed ?? true) || !(e.target instanceof Node) || !$('main.page')?.contains(e.target)) return;
  if (e.target instanceof Element && e.target.closest('a, button, summary, input, textarea')) return;
  const at = caret(e.clientX, e.clientY);
  const hit = at && [...on].find(([c, r]) => shown(c) && r.isPointInRange(...at))?.[0];
  if (!hit) return;
  const to = spotOf(hit);
  leader = 'page';
  if (to) { goal.page = scrollY + to.top - innerHeight * READING; glide(); }
  hit.classList.remove('flash');
  requestAnimationFrame(() => hit.classList.add('flash'));
});
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
  $('#show-resolved')?.addEventListener('change', () => { light(); again(); });
  light();
  place();
}
document.addEventListener('DOMContentLoaded', hook);
document.addEventListener('kb-refresh', hook);
addEventListener('resize', again);
