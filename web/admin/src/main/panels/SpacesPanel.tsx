import { useEffect, useState } from 'react';
import { api } from '../../shared/api';

type Props = { refreshKey: number; onError: (e: unknown) => void };

export function SpacesPanel({ refreshKey, onError }: Props) {
  const [list, setList] = useState('…');
  const [name, setName] = useState('');
  const [visibility, setVisibility] = useState('shared');

  async function load() {
    try {
      const out = await api<{
        spaces?: Array<{
          name?: string;
          builtin?: boolean;
          well_known?: boolean;
          visibility?: string;
          encryption?: string;
          retention?: string;
          convention?: string;
        }>;
      }>('/admin/api/spaces');
      const spaces = out.spaces || [];
      setList(
        spaces.length
          ? spaces
              .map((s) => {
                const kind = s.builtin
                  ? 'builtin'
                  : s.well_known
                    ? 'well-known'
                    : 'custom';
                let line = `${s.name} · ${kind} · ${s.visibility} · enc=${
                  s.encryption || 'none'
                } · ret=${s.retention || 'none'}`;
                if (s.convention) line += ` · ${s.convention}`;
                return line;
              })
              .join('\n')
          : '(no spaces)',
      );
    } catch (e) {
      setList(String((e as Error).message || e));
    }
  }

  useEffect(() => {
    void load();
  }, [refreshKey]);

  async function declareSpace() {
    const n = name.trim();
    if (!n) {
      alert('请输入空间名称');
      return;
    }
    try {
      const out = await api<{ name?: string; visibility?: string }>(
        '/admin/api/spaces',
        {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ name: n, visibility }),
        },
      );
      alert(`Space declared: ${out.name} (${out.visibility})`);
      await load();
    } catch (e) {
      onError(e);
    }
  }

  return (
    <section className="panel">
      <div className="section-head">
        <h2>Spaces（空间属性）</h2>
        <button type="button" onClick={() => void load()}>
          刷新
        </button>
      </div>
      <div className="row">
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
        <button type="button" onClick={() => void declareSpace()}>
          声明自定义空间
        </button>
      </div>
      <pre className="block muted" style={{ marginTop: '0.5rem' }}>
        {list}
      </pre>
      <p className="muted">
        空间 = 属性驱动分区（visibility / encryption / retention /
        import_grant）；四内置空间不可重声明。启动时自动种子{' '}
        <code>memory</code>（蒸馏记忆，默认 private）。
      </p>
    </section>
  );
}
