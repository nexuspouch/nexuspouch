export function $(id: string): HTMLElement {
  const el = document.getElementById(id);
  if (!el) throw new Error(`missing #${id}`);
  return el;
}

export function esc(s: unknown): string {
  return String(s ?? '')
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;');
}

export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  cls?: string,
  text?: string | null,
): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
}

export function clear(el: HTMLElement): void {
  el.replaceChildren();
}

export function setMsg(err: unknown): void {
  $('msg').textContent = err ? String((err as Error).message || err) : '';
}

export function notice(text: string): void {
  const n = $('notice');
  if (text) {
    n.textContent = text;
    n.classList.add('show');
  } else {
    n.textContent = '';
    n.classList.remove('show');
  }
}

export function clearNotice(): void {
  notice('');
}
