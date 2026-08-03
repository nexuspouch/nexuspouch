import { useEffect, useState } from 'react';
import { api } from '../../shared/api';
import { shortId } from '../../shared/format';

type Props = {
  refreshKey: number;
  onError: (e: unknown) => void;
  onRefresh: () => Promise<void>;
};

export function AuditPanel({ refreshKey, onError, onRefresh }: Props) {
  const [auditLog, setAuditLog] = useState('…');
  const [tokenList, setTokenList] = useState('…');
  const [label, setLabel] = useState('');
  const [scope, setScope] = useState('read');

  async function loadAudit() {
    try {
      const out = await api<{
        entries?: Array<{
          ts_ms?: number;
          kind?: string;
          op?: string;
          caller?: string;
          trust?: string;
          message?: string;
        }>;
      }>('/admin/api/audit?limit=40');
      const entries = out.entries || [];
      setAuditLog(
        entries.length
          ? entries
              .map(
                (e) =>
                  `${e.ts_ms || ''} ${e.kind || ''} ${e.op || ''} caller=${shortId(
                    e.caller || '',
                  )} trust=${e.trust || ''} ${e.message || ''}`,
              )
              .join('\n')
          : '(empty)',
      );
    } catch (e) {
      setAuditLog(String((e as Error).message || e));
    }
  }

  async function loadTokens() {
    try {
      const out = await api<{
        tokens?: Array<{ id?: string; label?: string; scopes?: string[] }>;
      }>('/admin/api/tokens');
      const tokens = out.tokens || [];
      setTokenList(
        tokens.length
          ? tokens
              .map(
                (t) =>
                  `${t.id} · ${t.label} · [${(t.scopes || []).join(',')}]`,
              )
              .join('\n')
          : '(no scoped tokens)',
      );
    } catch (e) {
      setTokenList(String((e as Error).message || e));
    }
  }

  useEffect(() => {
    void loadAudit();
    void loadTokens();
  }, [refreshKey]);

  async function createToken() {
    try {
      const out = await api<{ token?: string }>('/admin/api/tokens', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          label: (label || 'api').trim(),
          scopes: [scope],
        }),
      });
      alert(`Token (copy now):\n${out.token}`);
      await loadTokens();
    } catch (e) {
      onError(e);
    }
  }

  async function runReprotect() {
    const password = prompt(
      '再保护密码（与 ShePaw 主密码一致可生成 mirror.tar.enc；留空则仅写清单）',
      '',
    );
    if (password === null) return;
    if (
      !confirm(
        password
          ? '生成加密再保护包（manifest.json + mirror.tar.enc）？'
          : '生成清单快照（无密文）？',
      )
    ) {
      return;
    }
    try {
      const out = await api('/admin/api/reprotect', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(password ? { password } : {}),
      });
      alert(`reprotect ok: ${JSON.stringify(out)}`);
      await onRefresh();
    } catch (e) {
      onError(e);
    }
  }

  return (
    <section className="panel">
      <div className="section-head">
        <h2>审计 / Token / 再保护</h2>
        <div className="row">
          <button
            type="button"
            onClick={() => {
              void loadAudit();
              void loadTokens();
            }}
          >
            刷新审计
          </button>
          <button type="button" onClick={() => void runReprotect()}>
            再保护镜像
          </button>
        </div>
      </div>
      <pre className="block muted">{auditLog}</pre>
      <div className="row" style={{ marginTop: '0.75rem' }}>
        <input
          placeholder="token 标签"
          style={{ minWidth: '8rem' }}
          value={label}
          onChange={(e) => setLabel(e.target.value)}
        />
        <select value={scope} onChange={(e) => setScope(e.target.value)}>
          <option value="read">read</option>
          <option value="write">write</option>
          <option value="events">events</option>
          <option value="admin">admin</option>
        </select>
        <button type="button" onClick={() => void createToken()}>
          创建 API Token
        </button>
      </div>
      <pre className="block muted" style={{ marginTop: '0.5rem' }}>
        {tokenList}
      </pre>
      <p className="muted">
        scoped token：read=只读 API/WebDAV；write=可写 store（不含 wipe/migrate）；events=SSE；admin=全权限。
      </p>
    </section>
  );
}
