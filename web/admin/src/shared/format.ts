export function fmtBytes(n: number): string {
  n = Number(n) || 0;
  if (n < 1024) return `${n} B`;
  if (n < 1048576) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1073741824) return `${(n / 1048576).toFixed(1)} MB`;
  return `${(n / 1073741824).toFixed(1)} GB`;
}

export function shortId(id: string): string {
  id = String(id || '');
  return id.length > 8 ? `${id.slice(0, 8)}…` : id;
}

export function fmtTime(ms: number): string {
  ms = Number(ms) || 0;
  if (!ms) return '—';
  return new Date(ms).toLocaleString();
}

export function fmtRelative(ms: number): string {
  ms = Number(ms) || 0;
  if (!ms) return '—';
  const diff = Date.now() - ms;
  if (diff < 60_000) return '刚刚';
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`;
  if (diff < 7 * 86_400_000) return `${Math.floor(diff / 86_400_000)} 天前`;
  return fmtTime(ms);
}

export function fmtDuration(ms: number | null | undefined): string {
  if (ms == null) return '—';
  ms = Number(ms) || 0;
  const m = Math.round(ms / 60000);
  if (m < 1) return '<1 分钟';
  if (m < 60) return `${m} 分钟`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h} 小时 ${m % 60} 分`;
  const d = Math.floor(h / 24);
  return `${d} 天 ${h % 24} 小时`;
}

export function toolName(content: string): string {
  const s = String(content || '');
  const i = s.indexOf(':');
  return i > 0 ? s.slice(0, i) : s.slice(0, 40);
}

export function escHtml(s: unknown): string {
  return String(s ?? '')
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;');
}
