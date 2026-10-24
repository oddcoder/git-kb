// Figures: click a diagram, of any plugin, to see it on its own, fitted to the screen. There, scroll or pinch to
// zoom toward the pointer, drag to pan, double-click to fit again, and Esc or a click beside it to close.
import { target } from './dom';

const view = document.createElement('dialog');
view.className = 'kb-zoom';
document.body.append(view);

let svg: SVGSVGElement | null = null;
let scale = 1, x = 0, y = 0;
let drag: { x: number; y: number; moved: boolean; onSvg: boolean } | null = null;
const apply = () => { if (svg) svg.style.transform = `translate(${x}px, ${y}px) scale(${scale})`; };
const fit = () => { scale = 1; x = y = 0; apply(); };

document.addEventListener('click', (e) => {
  const shown = target(e)?.closest('.kb-stage')?.querySelector('svg');
  if (!shown) return;
  svg = shown.cloneNode(true) as SVGSVGElement;
  view.replaceChildren(svg);
  view.showModal();
  fit();
});

view.addEventListener('wheel', (e) => {
  if (!svg) return;
  e.preventDefault();
  const next = Math.min(20, Math.max(1, scale * Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.0025))));
  // Keep the point under the pointer where it is: the SVG's untransformed corner is its box less the translation.
  const box = svg.getBoundingClientRect();
  const [ox, oy] = [box.left - x, box.top - y];
  x = e.clientX - ox - ((e.clientX - box.left) / scale) * next;
  y = e.clientY - oy - ((e.clientY - box.top) / scale) * next;
  scale = next;
  apply();
}, { passive: false });

view.addEventListener('pointerdown', (e) => {
  drag = { x: e.clientX - x, y: e.clientY - y, moved: false, onSvg: !!target(e)?.closest('svg') };
  view.setPointerCapture(e.pointerId);
});
view.addEventListener('pointermove', (e) => {
  if (!drag) return;
  drag.moved = true;
  [x, y] = [e.clientX - drag.x, e.clientY - drag.y];
  apply();
});
view.addEventListener('pointerup', () => {
  if (drag && !drag.moved && !drag.onSvg) view.close();
  drag = null;
});
view.addEventListener('dblclick', fit);
