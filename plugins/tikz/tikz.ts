// Diagrams are drawn in the theme's own colors. TeX writes every color into the SVG, and each is a tint of one of
// preamble.tex's inks in its paper: the ink, the paper, or a mix of the two. So each color is read back as an ink
// and how much of it, in OKLab, and drawn again as that much of the theme's ink in the theme's paper, for its light
// scheme and its dark one; CSS light-dark() picks one by the scheme. The theme's paper and inks are style.css's.
type Lab = [number, number, number];
type Rgb = [number, number, number];

const linear = (u: number) => (u <= 0.04045 ? u / 12.92 : ((u + 0.055) / 1.055) ** 2.4);

function oklab(rgb: Rgb): Lab {
  const [r, g, b] = rgb.map((u) => linear(u / 255)) as Rgb;
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  return [0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
    1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
    0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s];
}

// #rgb, #rrggbb or rgb(r, g, b), as three bytes; anything else, such as none, is left alone.
function parse(color: string | null | undefined): Rgb | null {
  const hex = /^#([\da-f]{3}|[\da-f]{6})$/i.exec(color?.trim() ?? '');
  if (hex) {
    const h = hex[1].length === 3 ? [...hex[1]].map((c) => c + c).join('') : hex[1];
    return [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16)) as Rgb;
  }
  const rgb = /^rgb\(\s*([\d.]+)[\s,]+([\d.]+)[\s,]+([\d.]+)\s*\)$/i.exec(color?.trim() ?? '');
  return rgb ? (rgb.slice(1).map(Number) as Rgb) : null;
}
const known = (color: string): Rgb => parse(color) ?? [0, 0, 0];

// The preamble's paper and inks, and the style.css variable each becomes. Its gray, for arrows, groups and their
// labels, is the theme's secondary text color, not its comment gray, so labels stay readable in dark themes. Black
// is what TeX leaves unset: text.
const PAPER = oklab(known('#f2f0eb'));
const INKS: [Lab, string][] = ([['#5a6066', '--ink-software'], ['#98663a', '--ink-io'], ['#3f6e96', '--ink-control'],
  ['#4c7a5c', '--ink-compute'], ['#6e5e90', '--ink-memory'], ['#85837c', '--muted'], ['#000000', '--text']] as const)
  .map(([c, v]) => [oklab(known(c)), v]);

// A theme's color, light-dark(a, b), as its light and dark colors in OKLab.
function schemes(name: string): [Lab, Lab] | null {
  const value = getComputedStyle(document.documentElement).getPropertyValue(name);
  const both = /light-dark\(\s*(#[\da-f]{3,6})\s*,\s*(#[\da-f]{3,6})\s*\)/i.exec(value);
  const one = parse(value);
  return both ? [oklab(known(both[1])), oklab(known(both[2]))] : one ? [oklab(one), oklab(one)] : null;
}

// A color as the ink it is a tint of and how much of that ink: the ink whose line from the paper passes nearest.
function tint(c: Lab): { name: string; t: number } {
  let best = { name: '--text', t: 1, miss: Infinity };
  for (const [ink, name] of INKS) {
    const d = ink.map((x, i) => x - PAPER[i]), e = c.map((x, i) => x - PAPER[i]);
    const t = Math.max(0, Math.min(1, e.reduce((s, x, i) => s + x * d[i], 0) / d.reduce((s, x) => s + x * x, 0)));
    const miss = Math.hypot(...e.map((x, i) => x - t * d[i]));
    if (miss < best.miss) best = { name, t, miss };
  }
  return best;
}

const css = ([L, a, b]: Lab) => `oklab(${L.toFixed(3)} ${a.toFixed(4)} ${b.toFixed(4)})`;
const mix = (p: Lab, i: Lab, t: number) => p.map((x, k) => x + t * (i[k] - x)) as Lab;

// A diagram's colors in the current theme: each node's own color is kept, the first time, to draw it again from.
function theme(svg: SVGElement): void {
  const paper = schemes('--page');
  if (!paper) return;
  const inks = Object.fromEntries(INKS.map(([, name]) => [name, schemes(name)]));
  const again = (color: string | undefined): string | null => {
    const rgb = parse(color);
    if (!rgb) return null;
    const { name, t } = tint(oklab(rgb));
    const ink = inks[name];
    if (!ink) return null;
    return `light-dark(${css(mix(paper[0], ink[0], t))}, ${css(mix(paper[1], ink[1], t))})`;
  };
  svg.dataset.kbFill ??= svg.style.fill || '#000000';
  svg.style.fill = again(svg.dataset.kbFill) ?? '';
  for (const node of svg.querySelectorAll<SVGElement>('[fill], [stroke]')) {
    for (const prop of ['fill', 'stroke'] as const) {
      const key = prop === 'fill' ? 'kbFill' : 'kbStroke';
      node.dataset[key] ??= node.getAttribute(prop) ?? '';
      const value = again(node.dataset[key]);
      if (value) node.style[prop] = value;
    }
  }
}

// Each diagram as TikZJax draws it, and every one again in a theme chosen in the look menu. A drawn diagram is
// marked so, for live.js to keep while its source is the same.
document.addEventListener('tikzjax-load-finished', (e) => {
  if (!(e.target instanceof SVGElement)) return;
  theme(e.target);
  e.target.closest('[data-plugin]')?.setAttribute('data-rendered', '');
});
// The look is attributes of <html>, which look.ts sets as the reader chooses.
new MutationObserver(() => document.querySelectorAll<SVGElement>('[data-plugin="tikz"] .kb-stage svg').forEach(theme))
  .observe(document.documentElement, { attributeFilter: ['data-palette', 'data-scheme'] });

export {};
