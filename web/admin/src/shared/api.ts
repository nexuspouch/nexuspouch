export const TOKEN_KEY = 'shepaw_admin_token';

export function seedTokenFromQuery(): void {
  const t = new URLSearchParams(location.search).get('token');
  if (t) sessionStorage.setItem(TOKEN_KEY, t);
}

export function getToken(): string {
  return sessionStorage.getItem(TOKEN_KEY) || '';
}

export function setToken(token: string): void {
  sessionStorage.setItem(TOKEN_KEY, token.trim());
}

function authHeaders(extra?: HeadersInit): HeadersInit {
  const t = getToken();
  const h: Record<string, string> = { Accept: 'application/json' };
  if (t) h.Authorization = `Bearer ${t}`;
  return { ...h, ...(extra as Record<string, string> | undefined) };
}

export async function api<T = Record<string, unknown>>(
  path: string,
  opts: RequestInit = {},
): Promise<T> {
  const res = await fetch(path, {
    ...opts,
    headers: authHeaders(opts.headers),
  });
  if (res.status === 401) throw new Error('unauthorized — 检查 token');
  const data = (await res.json().catch(() => ({}))) as T & {
    message?: string;
    error?: string;
  };
  if (!res.ok) {
    throw new Error(data.message || data.error || res.statusText);
  }
  return data;
}

export async function postFeedback(
  query: string,
  hit: { uri: string; score_type?: string },
  rank: number,
  kind: string,
): Promise<void> {
  await api('/admin/api/sessions/feedback', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      kind,
      query,
      uri: hit.uri,
      rank,
      score_type: hit.score_type || undefined,
    }),
  });
}
