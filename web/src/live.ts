// The page, kept live: told by the server when a file of the site or a git ref changes, it fetches itself afresh and
// swaps in the parts that may differ, the sidebar, the page and its comments, rather than loading again. What the
// reader is doing is kept: where they are on the page, Page or Source, which threads are open, Show resolved, and
// diagrams drawn already, by any plugin, which are not drawn again unless their source changed; a plugin the page
// did not use before is loaded. While they write a comment, read a card, choose words or drag an edge, the change
// waits until they are done. The other scripts are told, by a kb-refresh event, to hook what is new.
import { $, $$ } from './dom';

// The parts that may differ, each with the part it follows, for one the page did not have before, as the column of
// comments when the first is begun.
const PARTS: [string, string][] = [['.side', ''], ['main.page', ''], ['.layout > .rail', 'main.page'],
  ['.layout > .edge-rail', '.layout > .rail']];

// Whether the reader is in the middle of something a fresh page would lose. A box they cannot type in, as the prompt
// to hand the page to an AI, holds text from the start, and is not their writing.
function busy(): boolean {
  const typing = $$<HTMLTextAreaElement>('textarea:not([readonly])')
    .some((t) => t.value.trim() || t === document.activeElement);
  const card = $(':popover-open');
  const choosing = !(getSelection()?.isCollapsed ?? true);
  return typing || !!card || choosing || document.documentElement.classList.contains('dragging-layout');
}

// Where the reader is: the first block in view, by the lines of the text it holds, as each block in the margin and
// each line of the source says, and how far down the screen it sits; so that an edit above it does not move it.
interface Place { line: number; source: boolean; top: number }
const firstLine = (b: HTMLElement) => Number((b.dataset.lines ?? '0').split('-')[0]);
function where(): Place | null {
  const first = $$('.blk[data-lines], .src-line').find((b) => b.getBoundingClientRect().bottom > 64);
  return first ? { line: firstLine(first), source: first.matches('.src-line'), top: first.getBoundingClientRect().top } : null;
}
function stay(at: Place | null): void {
  if (!at) return;
  const all = $$(at.source ? '.src-line' : '.blk[data-lines]');
  const same = all.find((b) => { const [a, z] = (b.dataset.lines ?? '').split('-').map(Number); return a <= at.line && at.line < z; })
    ?? all.find((b) => firstLine(b) >= at.line);
  if (same) scrollBy(0, same.getBoundingClientRect().top - at.top);
}

function swap(fresh: Document): void {
  const at = where();
  const source = $<HTMLInputElement>('#v-source')?.checked;
  const resolved = $<HTMLInputElement>('#show-resolved')?.checked;
  const open = new Set($$<HTMLDetailsElement>('details.thread[open]').map((d) => d.id));
  const closed = new Set($$<HTMLDetailsElement>('details.thread:not([open])').map((d) => d.id));
  // Diagrams drawn already, by their plugin and source, to put back in the same diagram undrawn.
  const key = (f: HTMLElement) => `${f.dataset.plugin}\n${$('details code', f)?.textContent}`;
  const drawn = new Map($$('figure.kb-block[data-rendered]').map((f) => [key(f), f]));
  for (const [part, after] of PARTS) {
    const now = $(part), next = $(part, fresh);
    if (now && next) now.replaceWith(next);
    else if (next && after) $(after)?.after(next);
    else if (now && !next) now.remove();
  }
  $('.layout')?.classList.toggle('has-rail', !!$('.layout > .rail'));
  document.title = fresh.title;
  // The drawing alone is put back: the fresh figure's bar, its number and caption, may have changed. Each drawing
  // once, as two figures of the same source are two drawings.
  for (const figure of $$('figure.kb-block')) {
    const old = drawn.get(key(figure));
    const stage = old && $('.kb-stage', old), undrawn = $('.kb-stage', figure);
    if (!stage || !undrawn) continue;
    drawn.delete(key(figure));
    undrawn.replaceWith(stage);
    figure.setAttribute('data-rendered', '');
  }
  const box = $<HTMLInputElement>('#v-source');
  if (box) box.checked = !!source;
  const show = $<HTMLInputElement>('#show-resolved');
  if (show) show.checked = !!resolved;
  for (const d of $$<HTMLDetailsElement>('details.thread')) {
    if (open.has(d.id)) d.open = true;
    if (closed.has(d.id)) d.open = false;
  }
  load(fresh);
  document.dispatchEvent(new Event('kb-refresh'));
  requestAnimationFrame(() => stay(at));
}

// The scripts and styles of plugins the fresh page uses and this one did not load yet, added in order, so that a
// plugin's own script runs after the library it draws with.
function load(fresh: Document): void {
  const have = new Set([...document.head.children].map((e) => e.outerHTML));
  const plugins = new Set($$('[data-plugin]').map((e) => e.dataset.plugin ?? ''));
  const loaded = new Set($$('[data-kb-plugin], [data-kb-plugin-head]', document.head)
    .map((e) => e.dataset.kbPlugin ?? e.dataset.kbPluginHead));
  for (const name of plugins) {
    if (loaded.has(name)) continue;
    const quoted = CSS.escape(name);
    for (const old of $$(`[data-kb-plugin-head="${quoted}"], [data-kb-plugin="${quoted}"]`, fresh.head)) {
      if (have.has(old.outerHTML)) continue;
      let e: Element;
      if (old instanceof HTMLScriptElement) {
        const s = document.createElement('script');
        for (const a of old.attributes) s.setAttribute(a.name, a.value);
        s.textContent = old.textContent;
        s.async = false;
        e = s;
      } else e = old.cloneNode(true) as Element;
      document.head.append(e);
    }
  }
}

// One fetch at a time, so that a page that takes the server a while cannot come back after a fresher one and be
// swapped in over it: a change while one is fetched has the page fetched once more after it. A change the page shows
// already is not fetched for: the server says when it saw each change, and when it began each page it sends, and a
// page begun after a change has it, as the page fetched after a reply sent from it has the reply.
let waiting: ReturnType<typeof setTimeout> | undefined;
let fetching = false, again = false, surely = false;
let told = 0, shown = 0;
async function refresh(sure = false): Promise<void> {
  waiting = undefined;
  if (fetching) {
    again = true;
    surely ||= sure;
    return;
  }
  if (!sure && told <= shown) return;
  if (busy()) {
    waiting = setTimeout(() => void refresh(sure), 1000);
    return;
  }
  fetching = true;
  try {
    const res = await fetch(location.href, { cache: 'no-store' }).catch(() => null);
    if (!res) return;
    const begun = Number(res.headers.get('x-kb-begun')) || 0;
    const fresh = new DOMParser().parseFromString(await res.text(), 'text/html');
    if (busy()) {
      waiting = setTimeout(() => void refresh(sure), 1000);
      return;
    }
    swap(fresh);
    shown = Math.max(shown, begun);
  } finally {
    fetching = false;
    if (again) {
      const sure = surely;
      again = surely = false;
      clearTimeout(waiting);
      void refresh(sure);
    }
  }
}

// The page loaded again as a whole, once the reader is done, as the server began again, as another version of git-kb or
// with other settings or plugins, so that its scripts, styles and look may differ.
function reload(): void {
  if (busy()) setTimeout(reload, 1000);
  else location.reload();
}

// A change the page made itself, as a reply sent from it: fetched afresh at once, rather than when the server tells of it.
document.addEventListener('kb-stale', (e) => {
  e.preventDefault();
  clearTimeout(waiting);
  void refresh(true);
});

// Changes come in bursts, as a save writes a file twice or a commit moves several refs: one fetch once they settle.
const events = new EventSource('/_events');
events.addEventListener('message', (e) => {
  // When the server saw it; from a server that does not say, it is newer than any page.
  told = Math.max(told, Number(String(e.data).split(' ')[1]) || Infinity);
  clearTimeout(waiting);
  waiting = setTimeout(() => void refresh(), 300);
});
// The server says which run of git-kb it is each time the page connects. Connecting again, after it stopped, the page
// loads again: as a whole if it began again since, or else fetched afresh, for what changed while it was away.
let server: string | undefined;
events.addEventListener('server', (e) => {
  const was = server;
  server = e.data;
  if (was === undefined) return;
  clearTimeout(waiting);
  if (was !== server) reload();
  else waiting = setTimeout(() => void refresh(true), 300);
});
