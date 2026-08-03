import { useEffect, useState } from 'react';
import { api } from '../../shared/api';
import { fmtRelative, shortId } from '../../shared/format';
import { useFeedback } from '../../shared/ui/feedback';

type AuditEntry = {
  ts_ms?: number;
  kind?: string;
  op?: string;
  caller?: string;
  trust?: string;
  message?: string;
};

type TokenRow = {
  id?: string;
  label?: string;
  scopes?: string[];
  created_ms?: number;
  agent_id?: string | null;
};

type Props = {
  refreshKey: number;
  onError: (e: unknown) => void;
  onRefresh: () => Promise<void>;
};

export function AuditPanel({ refreshKey, onError, onRefresh }: Props) {
  const { toast, confirm, prompt, revealSecret } = useFeedback();
  const [audit, setAudit] = useState<AuditEntry[] | null>(null);
  const [tokens, setTokens] = useState<TokenRow[] | null>(null);
  const [label, setLabel] = useState('');
  const [scope, setScope] = useState('read');
  const [busy, setBusy] = useState(false);
  const [loadErr, setLoadErr] = useState('');

  async function loadAudit() {
    try {
      const out = await api<{ entries?: AuditEntry[] }>('/admin/api/audit?limit=40');
      setAudit(out.entries || []);
    } catch (e) {
      setLoadErr(String((e as Error).message || e));
      setAudit([]);
    }
  }

  async function loadTokens() {
    try {
      const out = await api<{ tokens?: TokenRow[] }>('/admin/api/tokens');
      setTokens(out.tokens || []);
    } catch (e) {
      setLoadErr(String((e as Error).message || e));
      setTokens([]);
    }
  }

  useEffect(() => {
    void loadAudit();
    void loadTokens();
  }, [refreshKey]);

  async function createToken() {
    setBusy(true);
    try {
      const out = await api<{ token?: string }>('/admin/api/tokens', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          label: (label || 'api').trim(),
          scopes: [scope],
        }),
      });
      await revealSecret({
        title: 'API Token 已创建',
        secret: out.token || '',
        hint: '仅显示一次，关闭后无法再次查看。请立即复制保存。',
      });
      setLabel('');
      await loadTokens();
      toast('Token 已创建', { kind: 'ok' });
    } catch (e) {
      onError(e);
    } finally {
      setBusy(false);
    }
  }

  async function revokeToken(id: string, tokenLabel: string) {
    const ok = await confirm({
      title: '吊销 Token',
      message: `吊销 ${tokenLabel || id}？立即失效，不可恢复。`,
      confirmLabel: '吊销',
      danger: true,
    });
    if (!ok) return;
    setBusy(true);
    try {
      await api('/admin/api/tokens/revoke', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id }),
      });
      toast('Token 已吊销', { kind: 'ok' });
      await loadTokens();
    } catch (e) {
      onError(e);
    } finally {
      setBusy(false);
    }
  }

  async function runReprotect() {
    const password = await prompt({
      title: '再保护镜像',
      message:
        '输入再保护密码（与 ShePaw 主密码一致可生成 mirror.tar.enc；留空则仅写清单）',
      placeholder: '密码（可留空）',
      password: true,
      allowEmpty: true,
      confirmLabel: '继续',
    });
    if (password === null) return;

    const ok = await confirm({
      title: '确认再保护',
      message: password
        ? '生成加密再保护包（manifest.json + mirror.tar.enc）？'
        : '生成清单快照（无密文）？',
      confirmLabel: '开始',
    });
    if (!ok) return;

    setBusy(true);
    try {
      const out = await api('/admin/api/reprotect', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(password ? { password } : {}),
      });
      toast(`再保护完成：${JSON.stringify(out)}`, {
        kind: 'ok',
        durationMs: 6000,
      });
      await onRefresh();
    } catch (e) {
      onError(e);
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <section className="panel">
        <div className="section-head">
          <h2>API Token</h2>
          <div className="row">
            <button
              type="button"
              disabled={busy}
              onClick={() => {
                void loadAudit();
                void loadTokens();
              }}
            >
              刷新
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={() => void runReprotect()}
            >
              再保护镜像
            </button>
          </div>
        </div>
        <div className="row" style={{ marginBottom: '0.75rem' }}>
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
          <button
            type="button"
            className="primary"
            disabled={busy}
            onClick={() => void createToken()}
          >
            创建
          </button>
        </div>
        <p className="muted" style={{ marginTop: 0 }}>
          read=只读 · write=可写 store（不含 wipe/migrate）· events=SSE · admin=全权限
        </p>
        {loadErr ? <p className="err">{loadErr}</p> : null}
        {tokens === null ? (
          <p className="muted">加载中…</p>
        ) : !tokens.length ? (
          <p className="muted">暂无 scoped token</p>
        ) : (
          <table className="data">
            <thead>
              <tr>
                <th>标签</th>
                <th>作用域</th>
                <th>创建</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {tokens.map((t) => (
                <tr key={t.id || t.label}>
                  <td>
                    <div>{t.label || '—'}</div>
                    <div className="muted mono">{shortId(t.id || '')}</div>
                    {t.agent_id ? (
                      <div className="muted">agent {shortId(t.agent_id)}</div>
                    ) : null}
                  </td>
                  <td>
                    <div className="chip-row">
                      {(t.scopes || []).map((s) => (
                        <span key={s} className="badge accent">
                          {s}
                        </span>
                      ))}
                    </div>
                  </td>
                  <td>{fmtRelative(Number(t.created_ms))}</td>
                  <td>
                    <button
                      type="button"
                      className="danger"
                      disabled={busy || !t.id}
                      onClick={() => void revokeToken(t.id || '', t.label || '')}
                    >
                      吊销
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>

      <section className="panel">
        <div className="section-head">
          <h2>审计日志</h2>
          <button type="button" onClick={() => void loadAudit()}>
            刷新
          </button>
        </div>
        {audit === null ? (
          <p className="muted">加载中…</p>
        ) : !audit.length ? (
          <p className="muted">暂无记录</p>
        ) : (
          <div className="table-scroll">
            <table className="data">
              <thead>
                <tr>
                  <th>时间</th>
                  <th>操作</th>
                  <th>调用方</th>
                  <th>详情</th>
                </tr>
              </thead>
              <tbody>
                {audit.map((e, i) => (
                  <tr key={`${e.ts_ms}-${e.op}-${i}`}>
                    <td className="nowrap">{fmtRelative(Number(e.ts_ms))}</td>
                    <td>
                      <span className="badge">{e.kind || '—'}</span>{' '}
                      <code>{e.op || '—'}</code>
                    </td>
                    <td>
                      {shortId(e.caller || '')}
                      {e.trust ? (
                        <div className="muted">{e.trust}</div>
                      ) : null}
                    </td>
                    <td className="muted">{e.message || '—'}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>
    </>
  );
}
