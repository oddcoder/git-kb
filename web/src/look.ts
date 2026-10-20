// The look of the pages: light or dark, a theme of colors, the text's size, and the fonts of text and code, each
// an attribute of the page's <html>, as data-palette="nord", which style.css reads. The site's is what
// git-kb.toml's [look] gives the page; a reader may choose their own, in the Aa menu this puts in the bar, kept in
// their browser and set before the page is drawn, as this script is in its head.

/** Each part of the look, by its attribute: its heading in the menu, its choices, and its default. */
const PARTS: Record<string, { heading: string; choices: [value: string, label: string][]; default: string }> = {
  scheme: { heading: 'Scheme', choices: [['light', '☀ Light'], ['auto', '◐ Auto'], ['dark', '☾ Dark']], default: 'auto' },
  palette: {
    heading: 'Theme',
    choices: [['github', 'GitHub'], ['paper', 'Paper'], ['solarized', 'Solarized'], ['nord', 'Nord'], ['gruvbox', 'Gruvbox']],
    default: 'github',
  },
  size: { heading: 'Text size', choices: [['14', 'S'], ['15', 'M'], ['16', 'L'], ['18', 'XL'], ['20', 'XXL']], default: '16' },
  font: { heading: 'Text font', choices: [['sans', 'Sans'], ['serif', 'Serif'], ['mono', 'Mono']], default: 'sans' },
  code: { heading: 'Code font', choices: [['system', 'System'], ['menlo', 'Menlo'], ['courier', 'Courier']], default: 'system' },
};

const root = document.documentElement;
const KEPT = 'git-kb-look';
const offered = (key: string, value: unknown): value is string =>
  typeof value === 'string' && (PARTS[key]?.choices.some(([v]) => v === value) ?? false);

// The site's look, from its attributes; a part that is not one of its choices is its default.
const site: Record<string, string> = {};
for (const [key, part] of Object.entries(PARTS)) {
  const value = root.dataset[key];
  if (value !== undefined && !offered(key, value)) console.warn(`git-kb: [look] ${key} = "${value}" is not one of the menu's choices`);
  site[key] = offered(key, value) ? value : part.default;
}

function kept(): Record<string, unknown> {
  try {
    const look: unknown = JSON.parse(localStorage.getItem(KEPT) ?? '{}');
    return typeof look === 'object' && look !== null ? (look as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

// The reader's look, where they chose one the menu offers, and else the site's.
const look = kept();
for (const key of Object.keys(PARTS)) root.dataset[key] = offered(key, look[key]) ? look[key] : site[key];

function choose(key: string, value: string): void {
  root.dataset[key] = value;
  try {
    localStorage.setItem(KEPT, JSON.stringify({ ...kept(), [key]: value }));
  } catch {
    // A browser that keeps nothing, as for a page opened from disk in some, shows the choice until it leaves.
  }
}

function show(): void {
  for (const b of document.querySelectorAll<HTMLElement>('[data-look] button')) {
    const key = b.closest<HTMLElement>('[data-look]')?.dataset.look ?? '';
    b.setAttribute('aria-pressed', String(b.dataset.value === root.dataset[key]));
  }
}

/** The Aa menu: a button that opens, as a popover, a row of choices for each part of the look. */
function menu(): HTMLElement[] {
  const open = Object.assign(document.createElement('button'), { type: 'button', className: 'look-open', title: 'Text and colors', textContent: 'Aa' });
  open.setAttribute('popovertarget', 'look');
  const pop = Object.assign(document.createElement('div'), { id: 'look', className: 'look', popover: 'auto' });
  for (const [key, { heading, choices }] of Object.entries(PARTS)) {
    const row = Object.assign(document.createElement('div'), { className: 'choices' });
    row.dataset.look = key;
    for (const [value, label] of choices) {
      const b = Object.assign(document.createElement('button'), { type: 'button' });
      b.dataset.value = value;
      if (key === 'palette') {
        // The theme's own colors, from style.css: its page, in light and in dark, and its accent.
        const swatch = Object.assign(document.createElement('span'), { className: 'swatch' });
        swatch.dataset.palette = value;
        swatch.append(document.createElement('i'), document.createElement('i'));
        b.append(swatch);
      }
      b.append(label);
      b.addEventListener('click', () => {
        choose(key, value);
        show();
      });
      row.append(b);
    }
    pop.append(Object.assign(document.createElement('h3'), { textContent: heading }), row);
  }
  return [open, pop];
}

document.addEventListener('DOMContentLoaded', () => {
  document.querySelector('.bar')?.append(...menu());
  show();
});
