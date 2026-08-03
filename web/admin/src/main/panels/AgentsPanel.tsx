import { useEffect, useState } from 'react';
import { api } from '../../shared/api';
import { fmtBytes, fmtRelative, shortId } from '../../shared/format';
import { useFeedback } from '../../shared/ui/feedback';

type AgentRow = {
  id?: string;
  name?: string;
  scopes?: string[];
  status?: string;
  created_ms?: number;
  quota?: { bytes_used?: number; max_bytes?: number };
};

type Props = { refreshKey: number; onError: (e: unknown) => void };

function QuotaCell({
  used,
  max,
}: {
  used: number;
  max: number;
}) {
  const pct = max > 0 ? Math.min(100, Math.round((used / max) * 100)) : 0;
  const tone = pct >= 90 ? 'danger' : pct >= 70 ? 'warn' : '';
  return (
    <div className="quota-cell">
      <div className={`quota-bar ${tone}`}>
        <span style={{ width: `${pct}%` }} />
      </div>
      <div className="muted">
        {fmtBytes(used)} / {fmtBytes(max)}（{pct}%）
      </div>
    </div>
  );
}

export function AgentsPanel({ refreshKey, onError }: Props) {
  const { toast, confirm } = useFeedback();
  const [agents, setAgents] = useState<AgentRow[] | null>(null);
  const [name, setName] = useState('');
  const [scope, setScope] = useState('store:read');
  const [maxBytes, setMaxBytes] = useState('1073741824');
  const [busy, setBusy] = useState(false);

  async function load() {
    try {
      const out = await api<{ agents?: AgentRow[] }>('/admin/api/agents');
      setAgents(out.agents || []);
    } catch (e) {
      setAgents([]);
      onError(e);
    }
  }

  useEffect(() => {
    void load();
  }, [refreshKey]);

  async function create() {
    setBusy(true);
    try {
      const scopes = scope
        .split(',')
        .map((s) => s.trim())
        .filter(Boolean);
      const out = await api<{ id?: string }>('/admin/api/agents', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          name: (name || 'agent').trim(),
          scopes,
          max_bytes: parseInt(maxBytes || '0', 10),
        }),
      });
      toast(`Agent 已创建：${out.id}`, { kind: 'ok' });
      setName('');
      await load();
    } catch (e) {
      onError(e);
    } finally {
      setBusy(false);
    }
  }

  async function revoke(id: string, agentName: string) {
    const ok = await confirm({
      title: '停用 Agent',
      message: `停用 ${agentName || id}？之后带该身份的请求将被拒绝。`,
      confirmLabel: '停用',
      danger: true,
    });
    if (!ok) return;
    setBusy(true);
    try {
      await api('/admin/api/agents/revoke', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id }),
      });
      toast('Agent 已停用', { kind: 'ok' });
      await load();
    } catch (e) {
      onError(e);
    } finally {
      setBusy(false);
    }
  }

  async function resetQuota(id: string) {
    const ok = await confirm({
      title: '重置配额',
      message: `将 ${shortId(id)} 的已用字节清零？`,
      confirmLabel: '重置',
    });
    if (!ok) return;
    setBusy(true);
    try {
      await api('/admin/api/agents/reset-quota', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id }),
      });
      toast('配额已重置', { kind: 'ok' });
      await load();
    } catch (e) {
      onError(e);
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="panel">
      <div className="section-head">
        <h2>Agents</h2>
        <button type="button" onClick={() => void load()}>
          刷新
        </button>
      </div>
      <div className="row" style={{ marginBottom: '0.75rem' }}>
        <input
          placeholder="名称"
          style={{ minWidth: '8rem' }}
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        <select value={scope} onChange={(e) => setScope(e.target.value)}>
          <option value="store:read">store:read</option>
          <option value="store:write">store:write</option>
          <option value="store:write:artifacts">store:write:artifacts</option>
          <option value="store:write:files">store:write:files</option>
          <option value="store:write,store:delete">store:write + store:delete</option>
        </select>
        <input
          type="number"
          title="max_bytes"
          style={{ minWidth: '7rem' }}
          value={maxBytes}
          onChange={(e) => setMaxBytes(e.target.value)}
        />
        <button
          type="button"
          className="primary"
          disabled={busy}
          onClick={() => void create()}
        >
          创建
        </button>
      </div>
      <p className="muted" style={{ marginTop: 0 }}>
        agent 经 scoped token 的 agent_id 或 MCP --agent 绑定；越权返回 acl_denied /
        quota_exceeded。
      </p>
      {agents === null ? (
        <p className="muted">加载中…</p>
      ) : !agents.length ? (
        <p className="muted">暂无 agent</p>
      ) : (
        <table className="data">
          <thead>
            <tr>
              <th>名称</th>
              <th>状态</th>
              <th>作用域</th>
              <th>配额</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {agents.map((a) => {
              const used = Number(a.quota?.bytes_used) || 0;
              const max = Number(a.quota?.max_bytes) || 0;
              const active = (a.status || '') === 'active';
              return (
                <tr key={a.id || a.name}>
                  <td>
                    <div>{a.name || '—'}</div>
                    <div className="muted mono">{shortId(a.id || '')}</div>
                    <div className="muted">{fmtRelative(Number(a.created_ms))}</div>
                  </td>
                  <td>
                    <span className={`badge ${active ? 'ok' : ''}`}>
                      {a.status || '—'}
                    </span>
                  </td>
                  <td>
                    <div className="chip-row">
                      {(a.scopes || []).map((s) => (
                        <span key={s} className="badge accent">
                          {s}
                        </span>
                      ))}
                    </div>
                  </td>
                  <td>
                    <QuotaCell used={used} max={max} />
                  </td>
                  <td>
                    <div className="row">
                      <button
                        type="button"
                        disabled={busy || !a.id}
                        onClick={() => void resetQuota(a.id || '')}
                      >
                        重置配额
                      </button>
                      {active ? (
                        <button
                          type="button"
                          className="danger"
                          disabled={busy || !a.id}
                          onClick={() => void revoke(a.id || '', a.name || '')}
                        >
                          停用
                        </button>
                      ) : null}
                    </div>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </section>
  );
}
