// WaveDrom diagrams: a timing diagram ({signal: [...]}), a register's bitfields ({reg: [...]}) or a logic circuit
// ({assign: [...]}), drawn in the theme's colors. WaveDrom draws in black on white with a few pale fills; here its
// black becomes the theme's text, its grays the theme's secondary text, its white the paper, and each other color
// stays itself in a light scheme and becomes a dark one of the same hue in a dark scheme, by CSS light-dark().
interface WaveJson { config?: { skin?: string }; [key: string]: unknown }
declare global {
  interface Window {
    JSON5?: { parse(text: string): unknown };
    WaveDrom?: { RenderWaveForm(index: number, source: WaveJson, output: string): void };
  }
}
type Lab = [number, number, number];

const linear = (u: number) => (u <= 0.04045 ? u / 12.92 : ((u + 0.055) / 1.055) ** 2.4);
function oklab([r8, g8, b8]: number[]): Lab {
  const [r, g, b] = [r8, g8, b8].map((u) => linear(u / 255));
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  return [0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
    1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
    0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s];
}
// Any CSS color, as bytes and alpha, by asking a canvas, which knows every color name; none, transparent and the
// like as null.
const canvas = document.createElement('canvas').getContext('2d');
function rgba(color: string): [number[], number] | null {
  const c = color.trim().toLowerCase();
  if (!canvas || !c || c === 'none' || c === 'transparent' || c.startsWith('url(') || c.startsWith('var(') || c === 'currentcolor') return null;
  canvas.fillStyle = '#010203';
  canvas.fillStyle = c;
  const got = String(canvas.fillStyle);
  if (got === '#010203' && c !== '#010203') return null;
  const hex = /^#([\da-f]{2})([\da-f]{2})([\da-f]{2})$/.exec(got);
  if (hex) return [[1, 2, 3].map((k) => parseInt(hex[k], 16)), 1];
  const m = /^rgba\(\s*([\d.]+),\s*([\d.]+),\s*([\d.]+),\s*([\d.]+)\s*\)$/.exec(got);
  return m ? [[Number(m[1]), Number(m[2]), Number(m[3])], Number(m[4])] : null;
}
const lab = ([L, a, b]: Lab) => `oklab(${L.toFixed(3)} ${a.toFixed(4)} ${b.toFixed(4)})`;

// A WaveDrom color in the theme: black, grays and white as the theme's; any other in its own hue, for each scheme.
function themed(color: string): string | null {
  const parsed = rgba(color);
  if (!parsed || parsed[1] === 0) return null;
  const [L, a, b] = oklab(parsed[0]);
  if (Math.hypot(a, b) < 0.02) {
    // White is the paper: none, so the figure's own shows through.
    if (L > 0.95) return 'transparent';
    if (L < 0.3) return 'var(--text)';
    return 'var(--muted)';
  }
  // Pale fills turn deep, and deep strokes and text turn light, so each reads on a dark paper as on a light one.
  const dark: Lab = [Math.min(0.85, Math.max(0.32, 1.08 - L)), a * 0.8, b * 0.8];
  return `light-dark(${lab([L, a, b])}, ${lab(dark)})`;
}

// The colors in CSS declarations, fill: #000 and the like, in the theme.
const COLOR = /(fill|stroke|color)\s*:\s*([^;}]+)/g;
const recolor = (css: string) => css.replace(COLOR, (whole, prop: string, value: string) => {
  const to = themed(value);
  return to ? `${prop}:${to}` : whole;
});

// The skin's styles: its colors in the theme, and its rules for a kind of element, such as text{...}, which would
// reach every SVG on the page, limited to this diagram. Its rules for its own classes stay as they are, as the
// shapes WaveDrom draws by <use> take them from where they are defined.
function restyle(svg: SVGSVGElement): void {
  const id = `#${CSS.escape(svg.id)}`;
  for (const style of svg.querySelectorAll('style')) {
    style.textContent = recolor(style.textContent ?? '').split('}').map((rule) => {
      const [selectors, body] = rule.split('{');
      if (body === undefined) return rule;
      return `${selectors.split(',').map((s) => (/^\s*[a-z]/i.test(s) ? `${id} ${s.trim()}` : s)).join(',')}{${body}`;
    }).join('}');
  }
}

// The colors WaveDrom writes on its elements, as attributes and styles, in the theme.
function theme(svg: SVGSVGElement): void {
  const [width, height] = [svg.getAttribute('width'), svg.getAttribute('height')];
  if (!svg.hasAttribute('viewBox') && width && height) svg.setAttribute('viewBox', `0 0 ${parseFloat(width)} ${parseFloat(height)}`);
  for (const node of svg.querySelectorAll<SVGElement>('*')) {
    for (const prop of ['fill', 'stroke'] as const) {
      const given = node.getAttribute(prop);
      const to = given && themed(given);
      if (to) node.setAttribute(prop, to);
    }
    const inline = node.getAttribute('style');
    if (inline) node.setAttribute('style', recolor(inline));
  }
  // Text WaveDrom leaves black by default, as a register's labels, in the theme's text color.
  for (const text of svg.querySelectorAll<SVGElement>('text, tspan')) {
    if (text.closest('defs')) continue;
    const to = themed(getComputedStyle(text).fill);
    if (to) text.style.fill = to;
  }
}

let n = 0;
function draw(): void {
  const { JSON5, WaveDrom } = window;
  if (!JSON5 || !WaveDrom) return;
  for (const box of document.querySelectorAll<HTMLElement>('[data-plugin="wavedrom"] .kb-wavedrom')) {
    const figure = box.closest('[data-plugin]');
    const pre = box.querySelector<HTMLElement>('.kb-wavedrom-source');
    if (!figure || !pre || figure.hasAttribute('data-rendered')) continue;
    const out = document.createElement('div');
    out.className = 'kb-wavedrom-out';
    out.id = `kb-wavedrom-${++n}`;
    box.append(out);
    try {
      const source = JSON5.parse(pre.textContent ?? '');
      if (typeof source !== 'object' || source === null || Array.isArray(source)) {
        throw new Error('a diagram is an object, such as {signal: [...]} or {reg: [...]}');
      }
      WaveDrom.RenderWaveForm(n, source as WaveJson, 'kb-wavedrom-');
      const svg = out.querySelector('svg');
      if (!svg) throw new Error('WaveDrom drew nothing: give signal, reg or assign');
      svg.id ||= `${out.id}-svg`;
      restyle(svg);
      theme(svg);
      box.classList.add('drawn');
      pre.classList.remove('kb-error');
      figure.setAttribute('data-rendered', '');
    } catch (e) {
      out.remove();
      pre.classList.add('kb-error');
      pre.title = e instanceof Error ? e.message : String(e);
    }
  }
}

if (document.readyState === 'loading') addEventListener('DOMContentLoaded', draw);
else draw();
document.addEventListener('kb-refresh', draw);
// A new theme's paper and text are the same var()s, and light-dark() follows the scheme, so nothing need be redrawn.

export {};
