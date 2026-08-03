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
  let res: Response;
  try {
    res = await fetch(path, {
      ...opts,
      headers: authHeaders(opts.headers),
    });
  } catch {
    throw new Error('无法连接节点 — 请先启动 nexuspouch（默认 :8787）');
  }
  if (res.status === 401) throw new Error('unauthorized — 检查 token');
  // Vite 代理在后端未启动时会返回 500/502 + 纯文本，勿当业务错误。
  if (res.status === 502 || res.status === 503 || res.status === 504) {
    throw new Error('节点不可达 — 请确认 nexuspouch 已在 :8787 监听');
  }
  const ct = res.headers.get('content-type') || '';
  if (!ct.includes('application/json')) {
    if (!res.ok) {
      throw new Error(
        res.status >= 500
          ? '节点 API 失败 — 开发时请先 `cargo run -- --listen 127.0.0.1:8787`（Vite 会代理 /admin/api）'
          : `${res.status} ${res.statusText}`,
      );
    }
  }
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
