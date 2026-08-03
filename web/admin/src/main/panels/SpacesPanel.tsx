import { useEffect, useState } from 'react';
import { api } from '../../shared/api';
import { useFeedback } from '../../shared/ui/feedback';

type SpaceRow = {
  name?: string;
  builtin?: boolean;
  well_known?: boolean;
  visibility?: string;
  encryption?: string;
  retention?: string;
  convention?: string;
};

type Props = { refreshKey: number; onError: (e: unknown) => void };

function kindOf(s: SpaceRow): string {
  if (s.builtin) return 'builtin';
  if (s.well_known) return 'well-known';
  return 'custom';
}

export function SpacesPanel({ refreshKey, onError }: Props) {
  const { toast } = useFeedback();
  const [spaces, setSpaces] = useState<SpaceRow[] | null>(null);
  const [name, setName] = useState('');
  const [visibility, setVisibility] = useState('shared');
  const [busy, setBusy] = useState(false);

  async function load() {
    try {
      const out = await api<{ spaces?: SpaceRow[] }>('/admin/api/spaces');
      setSpaces(out.spaces || []);
    } catch (e) {
      setSpaces([]);
      onError(e);
    }
  }

  useEffect(() => {
    void load();
  }, [refreshKey]);

  async function declareSpace() {
    const n = name.trim();
    if (!n) {
      toast('请输入空间名称', { kind: 'err' });
      return;
    }
    setBusy(true);
    try {
      const out = await api<{ name?: string; visibility?: string }>(
        '/admin/api/spaces',
        {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ name: n, visibility }),
        },
      );
      toast(`已声明空间 ${out.name}（${out.visibility}）`, { kind: 'ok' });
      setName('');
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
        <h2>Spaces</h2>
        <button type="button" onClick={() => void load()}>
          刷新
        </button>
      </div>
      <div className="row" style={{ marginBottom: '0.75rem' }}>
        <input
          placeholder="名称（小写字母开头）"
          style={{ minWidth: '8rem' }}
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        <select
          value={visibility}
          onChange={(e) => setVisibility(e.target.value)}
        >
          <option value="shared">shared</option>
          <option value="private">private</option>
        </select>
        <button
          type="button"
          className="primary"
          disabled={busy}
          onClick={() => void declareSpace()}
        >
          声明自定义空间
        </button>
      </div>
      <p className="muted" style={{ marginTop: 0 }}>
        空间 = 属性驱动分区；四内置空间不可重声明。启动时自动种子{' '}
        <code>memory</code>（默认 private）。
      </p>
      {spaces === null ? (
        <p className="muted">加载中…</p>
      ) : !spaces.length ? (
        <p className="muted">暂无空间</p>
      ) : (
        <table className="data">
          <thead>
            <tr>
              <th>名称</th>
              <th>类型</th>
              <th>可见性</th>
              <th>加密</th>
              <th>保留</th>
              <th>约定</th>
            </tr>
          </thead>
          <tbody>
            {spaces.map((s) => (
              <tr key={s.name}>
                <td>
                  <code>{s.name}</code>
                </td>
                <td>
                  <span className="badge">{kindOf(s)}</span>
                </td>
                <td>{s.visibility || '—'}</td>
                <td>{s.encryption || 'none'}</td>
                <td>{s.retention || 'none'}</td>
                <td className="muted">{s.convention || '—'}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}
