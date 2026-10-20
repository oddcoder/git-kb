// The page, kept live: told by the server that the site has changed, it fetches itself afresh and swaps in the parts
// that may differ, the sidebar and the page, rather than loading again. What the reader is doing is kept: where they
// are on the page, and Page or Source. While they read a card, choose words or drag an edge, the change waits until
// they are done. The other scripts are told, by a kb-refresh event, to hook what is new.
import { $, $$ } from './dom';

const PARTS = ['.side', 'main.page'];

function busy(): boolean {
  const card = $(':popover-open');
  const choosing = !(getSelection()?.isCollapsed ?? true);
  return !!card || choosing || document.documentElement.classList.contains('dragging-layout');
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
  for (const part of PARTS) {
    const now = $(part), next = $(part, fresh);
    if (now && next) now.replaceWith(next);
  }
  document.title = fresh.title;
  const box = $<HTMLInputElement>('#v-source');
  if (box) box.checked = !!source;
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

// Changes come in bursts, as a save that writes a file twice: one fetch once they settle.
new EventSource('/_events').addEventListener('message', () => {
  clearTimeout(waiting);
  waiting = setTimeout(refresh, 300);
});
