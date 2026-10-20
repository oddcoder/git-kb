// Search in a built site, where there is no server to ask: the pages that hold every word of the query, best first,
// scored as the server scores them. A page scores for each occurrence of a word: most in its title, more in a
// heading than in the text. Each hit points to the page's best-matching section. The index is _search.js's.
import { $ } from './dom';

/** A page in the index: its path, its file, its title, and its sections as [heading id, heading, text]. */
interface Indexed { path: string; href: string; title: string; sections: [string | null, string, string][] }
declare global { interface Window { KB_SEARCH?: Indexed[] } }

const escape = (s: string) => s.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c] ?? c);
const occurrences = (hay: string, word: string) => hay.toLowerCase().split(word).length - 1;

// Text as HTML with each occurrence of the words marked.
function mark(text: string, words: string[]): string {
  if (!words.length) return escape(text);
  const pattern = new RegExp(`(${words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('|')})`, 'gi');
  return text.split(pattern).map((part, i) => (i % 2 ? `<mark>${escape(part)}</mark>` : escape(part))).join('');
}

// About 220 characters of text around the first of the words, with every word marked.
function passage(text: string, words: string[]): string {
  const lower = text.toLowerCase();
  const first = Math.min(...words.map((w) => lower.indexOf(w)).filter((i) => i >= 0), text.length);
  const from = Math.max(0, (first === text.length ? 0 : first) - 60), to = Math.min(text.length, from + 220);
  return `${from > 0 ? '…' : ''}${mark(text.slice(from, to).trim(), words)}${to < text.length ? '…' : ''}`;
}

function search(query: string): void {
  const said = $('#kb-search-said'), list = $('#kb-search-results');
  if (!said || !list) return;
  const words = query.split(/\s+/).filter(Boolean).map((w) => w.toLowerCase());
  const root = new URL('.', location.href).href;
  const count = (hay: string) => words.reduce((n, w) => n + occurrences(hay, w), 0);
  const hits = (window.KB_SEARCH ?? []).flatMap((page) => {
    const everything = [page.title, page.path, ...page.sections.flatMap(([, h, t]) => [h, t])].join(' ').toLowerCase();
    if (!words.length || !words.every((w) => everything.includes(w))) return [];
    const scores = page.sections.map(([, h, t]) => 5 * count(h) + count(t));
    const best = scores.reduce((b, s, i) => (s > scores[b] ? i : b), 0);
    return [{ page, section: page.sections[best], score: 20 * count(page.title) + scores.reduce((a, b) => a + b, 0) }];
  }).sort((a, b) => b.score - a.score || a.page.title.toLowerCase().localeCompare(b.page.title.toLowerCase()));
  const q = `“${escape(query)}”`;
  said.textContent = '';
  said.innerHTML = !words.length ? 'Type a word or two above.' : !hits.length ? `Nothing matches ${q}.`
    : hits.length === 1 ? `1 page matches ${q}.` : `${hits.length} pages match ${q}.`;
  list.innerHTML = hits.map(({ page, section }) => {
    const [id, heading, text] = section ?? [null, '', ''];
    const anchor = id ? `#${encodeURIComponent(id)}` : '';
    const under = heading && heading !== page.title ? ` <span class="muted">› ${mark(heading, words)}</span>` : '';
    return `<li><a href="${escape(root + page.href + anchor)}"><strong>${mark(page.title, words)}</strong>${under}</a>`
      + `<p>${passage(text, words)}</p><small>${escape(page.path)}</small></li>`;
  }).join('');
}

const query = new URLSearchParams(location.search).get('q') ?? '';
const box = $<HTMLInputElement>('form.search input[name="q"]');
if (box) box.value = query;
search(query);
