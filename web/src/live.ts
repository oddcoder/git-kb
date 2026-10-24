// The page, kept live: told by the server when a file of the site or a git ref changes, it fetches itself afresh and
// swaps in the parts that may differ, the sidebar, the page and its comments, rather than loading again. What the
// reader is doing is kept: where they are on the page, Page or Source, which threads are open, and Show resolved.
// While they write a comment, read a card, choose words or drag an edge, the change waits until they are done. The
// other scripts are told, by a kb-refresh event, to hook what is new.
import { $, $$ } from './dom';

// The parts that may differ, each with the part it follows, for one the page did not have before, as the column of
// comments when the first is begun.
const PARTS: [string, string][] = [['.side', ''], ['main.page', ''], ['.layout > .rail', 'main.page']];

function busy(): boolean {
  const typing = $$<HTMLTextAreaElement>('textarea').some((t) => t.value.trim() || t === document.activeElement);
  const card = $(':popover-open');
  const choosing = !(getSelection()?.isCollapsed ?? true);
  return typing || !!card || choosing || document.documentElement.classList.contains('dragging-layout');
}

// Where the reader is: the place on the page of the first block in view, and how far down the screen it sits.
interface Place { index: number; top: number }
const blocks = () => $$('.p-page > *, .p-source > *');
function where(): Place | null {
  const all = blocks();
  const index = all.findIndex((b) => b.getBoundingClientRect().bottom > 64);
  return index < 0 ? null : { index, top: all[index].getBoundingClientRect().top };
}
function stay(at: Place | null): void {
  const block = at && blocks()[at.index];
  if (block) scrollBy(0, block.getBoundingClientRect().top - at.top);
}

function swap(fresh: Document): void {
  const at = where();
  const source = $<HTMLInputElement>('#v-source')?.checked;
  const resolved = $<HTMLInputElement>('#show-resolved')?.checked;
  const open = new Set($$<HTMLDetailsElement>('details.thread[open]').map((d) => d.id));
  const closed = new Set($$<HTMLDetailsElement>('details.thread:not([open])').map((d) => d.id));
  for (const [part, after] of PARTS) {
    const now = $(part), next = $(part, fresh);
    if (now && next) now.replaceWith(next);
    else if (next && after) $(after)?.after(next);
    else if (now && !next) now.remove();
  }
  $('.layout')?.classList.toggle('has-rail', !!$('.layout > .rail'));
  document.title = fresh.title;
  const box = $<HTMLInputElement>('#v-source');
  if (box) box.checked = !!source;
  const show = $<HTMLInputElement>('#show-resolved');
  if (show) show.checked = !!resolved;
  for (const d of $$<HTMLDetailsElement>('details.thread')) {
    if (open.has(d.id)) d.open = true;
    if (closed.has(d.id)) d.open = false;
  }
  document.dispatchEvent(new Event('kb-refresh'));
  requestAnimationFrame(() => stay(at));
}

let waiting: ReturnType<typeof setTimeout> | undefined;
async function refresh(): Promise<void> {
  waiting = undefined;
  if (busy()) {
    waiting = setTimeout(refresh, 1000);
    return;
  }
  const res = await fetch(location.href, { cache: 'no-store' }).catch(() => null);
  if (!res) return;
  const fresh = new DOMParser().parseFromString(await res.text(), 'text/html');
  if (busy()) {
    waiting = setTimeout(refresh, 1000);
    return;
  }
  swap(fresh);
}

// Changes come in bursts, as a save writes a file twice or a commit moves several refs: one fetch once they settle.
new EventSource('/_events').addEventListener('message', () => {
  clearTimeout(waiting);
  waiting = setTimeout(refresh, 300);
});
