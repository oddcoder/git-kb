// The events the page's scripts, and the libraries plugins load, tell each other by.
interface DocumentEventMap {
  /** live.js swapped in a fresh copy of the page. */
  'kb-refresh': Event;
  /** TikZJax drew a diagram; the event's target is its SVG. */
  'tikzjax-load-finished': Event;
}
