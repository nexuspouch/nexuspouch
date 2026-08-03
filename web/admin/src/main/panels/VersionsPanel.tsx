import { useEffect, useState } from 'react';
import { api } from '../../shared/api';

type PubRow = { uri?: string; versions?: number };

type Props = { refreshKey: number; onError?: (e: unknown) => void };

export function VersionsPanel({ refreshKey, onError }: Props) {
  const [keepLast, setKeepLast] = useState<number | null>(null);
  const [keepEnv, setKeepEnv] = useState('');
  const [published, setPublished] = useState<PubRow[] | null>(null);
  const [err, setErr] = useState('');

  async function load() {
    try {
      const out = await api<{
        keep_last?: number;
        keep_env?: string;
        published_count?: number;
        published?: PubRow[];
      }>('/admin/api/versions');
      setKeepLast(out.keep_last ?? null);
      setKeepEnv(out.keep_env || '');
      setPublished(out.published || []);
      setErr('');
    } catch (e) {
      setErr(String((e as Error).message || e));
      setPublished([]);
      onError?.(e);
    }
  }

  useEffect(() => {
    void load();
  }, [refreshKey]);

  return (
    <section className="panel">
      <div className="section-head">
        <h2>Versions / 发布产物</h2>
        <button type="button" onClick={() => void load()}>
          刷新
        </button>
      </div>
      <p className="muted" style={{ marginTop: 0 }}>
        全局保留：<code>NEXUSPOUCH_VERSIONS_KEEP</code>
        （默认 10；发布/protected 产物不修剪）。
      </p>
      <div className="stat-chips">
        <span className="stat-chip">
          keep_last <strong>{keepLast ?? '—'}</strong>
        </span>
        <span className="stat-chip">
          env <strong>{keepEnv || '—'}</strong>
        </span>
        <span className="stat-chip">
          published <strong>{published?.length ?? 0}</strong>
        </span>
      </div>
      {err ? <p className="err">{err}</p> : null}
      {published === null ? (
        <p className="muted">加载中…</p>
      ) : !published.length ? (
        <p className="muted">暂无已发布（protected）产物</p>
      ) : (
        <table className="data">
          <thead>
            <tr>
              <th>URI</th>
              <th>版本数</th>
            </tr>
          </thead>
          <tbody>
            {published.map((p) => (
              <tr key={p.uri}>
                <td className="mono">{p.uri || '—'}</td>
                <td>{p.versions ?? 0}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}
