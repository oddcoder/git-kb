// The layout, set by dragging and kept in this browser: the sidebar's width, or closed; the text column's width;
// and where the column sits, centred unless placed. It is set again on each page before it is drawn, as this script
// is in the page's head. Double-clicking an edge puts back its default.
import { $, stored } from './dom';

type Layout = Partial<Record<'side' | 'width' | 'left', number | null>>;
const root = document.documentElement;
// Only numbers are kept, so a value from an older layout, or one edited by hand, cannot break the page.
const KEY = 'git-kb-layout';
const load = (): Layout => Object.fromEntries(Object.entries(stored<Record<string, unknown>>(KEY, {}))
  .filter(([, v]) => Number.isFinite(v))) as Layout;
const save = (change: Layout) => {
  try {
    localStorage.setItem(KEY, JSON.stringify({ ...load(), ...change }));
  } catch {
    // A browser that keeps nothing, as for a page opened from disk in some, keeps the layout until it leaves.
  }
};
const px = (name: string, value: number | null | undefined) => {
  if (value == null) root.style.removeProperty(name); else root.style.setProperty(name, `${Math.round(value)}px`);
};

// The layout kept, set before the page is drawn, so that it does not jump.
const kept = load();
px('--text-width', kept.width);
px('--text-left', kept.left);
if (kept.side === 0) root.dataset.side = 'hidden'; else px('--side-width', kept.side);
if (Number.isFinite(kept.left)) root.dataset.placed = '';

const SIDE = { min: 160, max: 520, close: 110 };
const TEXT = { min: 360, snap: 32 };

interface Drag<S> {
  begin: () => S;
  move: (dx: number, start: S & { x: number }, x: number) => void;
  done: (start: S & { x: number }) => void;
  reset: () => void;
}
// An edge follows the pointer from where it was pressed, telling `move` how far it has gone, and `done` when let go.
function draggable<S extends object>(edge: HTMLElement, { begin, move, done, reset }: Drag<S>): void {
  let start: (S & { x: number }) | null = null;
  edge.addEventListener('pointerdown', (e) => {
    start = { x: e.clientX, ...begin() };
    edge.setPointerCapture(e.pointerId);
    edge.classList.add('dragging');
    root.classList.add('dragging-layout');
  });
  edge.addEventListener('pointermove', (e) => { if (start) move(e.clientX - start.x, start, e.clientX); });
  edge.addEventListener('pointerup', () => {
    if (!start) return;
    done(start);
    start = null;
    edge.classList.remove('dragging');
    root.classList.remove('dragging-layout');
  });
  edge.addEventListener('dblclick', reset);
}

// The edges, hooked when the page is ready, and again when live.ts swaps in a fresh copy of it, with edges of its own.
function hook(): void {
  const column = $('main.page');
  const columnBox = () => column?.getBoundingClientRect() ?? new DOMRect();
  // The text's own column of the layout: from the sidebar's right, or the layout's left when it is closed, to the
  // layout's right.
  const cell = () => {
    const layout = column?.parentElement?.getBoundingClientRect() ?? new DOMRect(0, 0, innerWidth);
    const side = $('.side');
    const open = side && getComputedStyle(side).display !== 'none';
    const left = open ? side.getBoundingClientRect().right : layout.left;
    return { left, width: layout.right - left };
  };
  const room = () => cell().width;
  const offsetInRoom = () => columnBox().left - cell().left;

  // The sidebar: dragged narrower than SIDE.close it closes, leaving its edge at the left to drag it open again.
  const side = $('.edge-side');
  if (side) {
    let width: number | null = null;
    draggable(side, {
      begin: () => ({}),
      move: (_dx, _start, x) => {
        width = Math.min(SIDE.max, x);
        if (width < SIDE.close) {
          root.dataset.side = 'hidden';
        } else {
          delete root.dataset.side;
          px('--side-width', Math.max(SIDE.min, width));
        }
      },
      done: () => {
        if (width == null) return;
        save({ side: width < SIDE.close ? 0 : Math.round(Math.max(SIDE.min, width)) });
        width = null;
      },
      reset: () => { delete root.dataset.side; px('--side-width', null); save({ side: null }); },
    });
  }

  // The text's right edge sets its width; a centred column grows on both sides, so by twice the pointer's travel.
  const right = $('.edge-right');
  if (right) {
    draggable(right, {
      begin: () => ({ width: columnBox().width, centred: !('placed' in root.dataset) }),
      move: (dx, start) => px('--text-width', Math.min(room(), Math.max(TEXT.min, start.width + dx * (start.centred ? 2 : 1)))),
      done: () => save({ width: Math.round(columnBox().width) }),
      reset: () => { px('--text-width', null); save({ width: null }); },
    });
  }

  // The text's left edge moves the column. Let go within TEXT.snap of the middle, it is centred, and stays centred
  // however wide the window; anywhere else it stays where it was put.
  const left = $('.edge-left');
  if (left) {
    const centre = () => { delete root.dataset.placed; px('--text-left', null); save({ left: null }); };
    draggable(left, {
      begin: () => ({ offset: offsetInRoom() }),
      move: (dx, start) => {
        const width = columnBox().width;
        root.dataset.placed = '';
        px('--text-left', Math.min(room() - width, Math.max(0, start.offset + dx)));
      },
      done: () => {
        const width = columnBox().width;
        const offset = offsetInRoom();
        if (Math.abs(offset - (room() - width) / 2) < TEXT.snap) centre();
        else save({ left: Math.round(offset) });
      },
      reset: centre,
    });
  }
}
document.addEventListener('DOMContentLoaded', hook);
document.addEventListener('kb-refresh', hook);

// A card resized by its corner, as a block's card in the margin, keeps that size: it opens at it next time.
const SIZE = 'git-kb-card-size';
type Size = { w: number; h: number };
document.addEventListener('beforetoggle', (e) => {
  const card = e.target;
  if (!(card instanceof HTMLElement) || !card.classList.contains('pop') || (e as ToggleEvent).newState !== 'open') return;
  const size = stored<Size | null>(SIZE, null);
  if (size && Number.isFinite(size.w) && Number.isFinite(size.h)) Object.assign(card.style, { width: `${size.w}px`, height: `${size.h}px` });
}, true);
document.addEventListener('toggle', (e) => {
  const card = e.target;
  if (!(card instanceof HTMLElement) || !card.classList.contains('pop') || (e as ToggleEvent).newState !== 'open') return;
  // Only a size the reader set by dragging is kept: a press on the card that ends with it another size than it opened.
  const opened = card.getBoundingClientRect();
  let pressed = false;
  const down = () => { pressed = true; };
  const up = () => {
    if (!pressed) return;
    pressed = false;
    const box = card.getBoundingClientRect();
    if (Math.abs(box.width - opened.width) > 2 || Math.abs(box.height - opened.height) > 2) {
      try {
        localStorage.setItem(SIZE, JSON.stringify({ w: Math.round(box.width), h: Math.round(box.height) }));
      } catch {
        // As above: kept until the page is left.
      }
    }
  };
  card.addEventListener('pointerdown', down);
  document.addEventListener('pointerup', up);
  card.addEventListener('toggle', (t) => {
    if ((t as ToggleEvent).newState === 'closed') { card.removeEventListener('pointerdown', down); document.removeEventListener('pointerup', up); }
  }, { once: true });
}, true);
