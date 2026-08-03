import { useEffect, useState } from 'react';
import { api } from '../../shared/api';

type Props = { refreshKey: number; onError: (e: unknown) => void };

export function AgentsPanel({ refreshKey, onError }: Props) {
  const [list, setList] = useState('…');
  const [name, setName] = useState('');
  const [scope, setScope] = useState('store:read');
  const [maxBytes, setMaxBytes] = useState('1073741824');

  async function load() {
    try {
      const out = await api<{
        agents?: Array<{
          id?: string;
          name?: string;
          scopes?: string[];
          status?: string;
          quota?: { bytes_used?: number; max_bytes?: number };
        }>;
      }>('/admin/api/agents');
      const agents = out.agents || [];
      setList(
        agents.length
          ? agents
              .map(
                (a) =>
                  `${a.id} · ${a.name} · [${(a.scopes || []).join(',')}] · ${
                    a.status
                  } · ${a.quota?.bytes_used || 0}/${a.quota?.max_bytes || 0}B`,
              )
              .join('\n')
          : '(no agents)',
      );
    } catch (e) {
      setList(String((e as Error).message || e));
    }
  }

  useEffect(() => {
    void load();
  }, [refreshKey]);

  async function create() {
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
      alert(`Agent created: ${out.id}`);
      await load();
    } catch (e) {
      onError(e);
    }
  }

  return (
    <section className="panel">
      <div className="section-head">
        <h2>Agents（agent 身份）</h2>
        <button type="button" onClick={() => void load()}>
          刷新
        </button>
      </div>
      <div className="row">
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
        <button type="button" onClick={() => void create()}>
          创建 Agent
        </button>
      </div>
      <pre className="block muted" style={{ marginTop: '0.5rem' }}>
        {list}
      </pre>
      <p className="muted">
        agent 经 scoped token 的 agent_id 或 MCP --agent 绑定；作用域外的操作返回
        acl_denied / quota_exceeded（审计记录）。
      </p>
    </section>
  );
}
