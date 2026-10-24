// Mermaid diagrams, drawn in the page's scheme, light or dark, and again when it changes. Each keeps its source,
// to draw from again.
const dark = (): boolean => {
  const scheme = document.documentElement.dataset.scheme;
  return scheme === 'dark' || (scheme !== 'light' && matchMedia('(prefers-color-scheme: dark)').matches);
};
let n = 0;

async function draw(again: boolean): Promise<void> {
  const mermaid = window.mermaid;
  if (!mermaid) return;
  mermaid.initialize({ startOnLoad: false, theme: dark() ? 'dark' : 'neutral', securityLevel: 'strict' });
  for (const pre of document.querySelectorAll<HTMLElement>('[data-plugin="mermaid"] .kb-mermaid')) {
    const figure = pre.closest('[data-plugin]');
    if (!figure || (figure.hasAttribute('data-rendered') && !again)) continue;
    pre.dataset.source ??= pre.textContent ?? '';
    try {
      const { svg } = await mermaid.render(`kb-mermaid-${++n}`, pre.dataset.source);
      pre.innerHTML = svg;
      pre.classList.remove('kb-error');
      figure.setAttribute('data-rendered', '');
    } catch (e) {
      pre.textContent = `${pre.dataset.source}\n\n${e instanceof Error ? e.message : String(e)}`;
      pre.classList.add('kb-error');
    }
  }
}

if (document.readyState === 'loading') addEventListener('DOMContentLoaded', () => void draw(false));
else void draw(false);
document.addEventListener('kb-refresh', () => void draw(false));
// The look is attributes of <html>, which look.ts sets as the reader chooses.
new MutationObserver(() => void draw(true)).observe(document.documentElement, { attributeFilter: ['data-scheme'] });
// On auto, the scheme is the system's, which may change while the page is open.
matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => void draw(true));

export {};
