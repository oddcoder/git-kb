// Search, by Pagefind: its look, on the search page, searching the site's bundle in the browser, served or built,
// started with what the bar's box asked; asking again there searches in place, rather than loading the page again.
import { $ } from './dom';

declare global {
  interface Window {
    PagefindUI?: new (options: Record<string, unknown>) => { triggerSearch: (q: string) => void };
  }
}

const root = $('#kb-search');
if (root && window.PagefindUI) {
  const ui = new window.PagefindUI({
    element: '#kb-search',
    // Both from the page's own address, as Pagefind takes a relative one from its script's.
    bundlePath: new URL(root.dataset.bundle ?? '/', location.href).pathname,
    baseUrl: new URL(root.dataset.base ?? '/', location.href).pathname,
    showSubResults: true,
    showImages: false,
    resetStyles: false,
  });
  const query = new URLSearchParams(location.search).get('q') ?? '';
  const box = $<HTMLInputElement>('form.search input[name="q"]');
  if (box) box.value = query;
  // Pagefind's own box says what was asked too.
  const said = () => $<HTMLInputElement>('#kb-search input');
  if (query) {
    ui.triggerSearch(query);
    const own = said();
    if (own) own.value = query;
  }
  $('form.search')?.addEventListener('submit', (e) => {
    e.preventDefault();
    const q = box?.value ?? '';
    history.replaceState(null, '', `?q=${encodeURIComponent(q)}`);
    ui.triggerSearch(q);
    const own = said();
    if (own) own.value = q;
  });
}
