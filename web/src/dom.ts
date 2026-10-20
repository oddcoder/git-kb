// Small helpers for finding things in the page, typed.

/** The first element matching a selector, as the type asked for. */
export const $ = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document): T | null =>
  root.querySelector<T>(selector);

/** Every element matching a selector, as an array. */
export const $$ = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document): T[] =>
  [...root.querySelectorAll<T>(selector)];

/** The element an event happened on, when it is one. */
export const target = (e: Event): Element | null => (e.target instanceof Element ? e.target : null);

/** A JSON value kept in this browser, or a fallback when there is none or it does not parse. */
export function stored<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : fallback;
  } catch {
    return fallback;
  }
}
