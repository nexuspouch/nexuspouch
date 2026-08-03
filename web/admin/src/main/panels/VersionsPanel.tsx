import { useEffect, useState } from 'react';
import { api } from '../../shared/api';

type Props = { refreshKey: number; onError?: (e: unknown) => void };

export function VersionsPanel({ refreshKey }: Props) {
  const [policy, setPolicy] = useState('…');
  const [published, setPublished] = useState('…');

  async function load() {
    try {
      const out = await api<{
        keep_last?: number;
        keep_env?: string;
        published_count?: number;
        published?: Array<{ uri?: string; versions?: number }>;
      }>('/admin/api/versions');
      setPolicy(
        `keep_last=${out.keep_last} (env ${out.keep_env}) · published=${
          out.published_count || 0
        }`,
      );
      const pub = out.published || [];
      setPublished(
        pub.length
          ? pub.map((p) => `${p.uri} · versions=${p.versions}`).join('\n')
          : '(no published/protected artifacts yet)',
      );
    } catch (e) {
      setPolicy(String((e as Error).message || e));
      setPublished('');
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
      <p className="muted">
        全局保留：<code>NEXUSPOUCH_VERSIONS_KEEP</code>
        （默认 10；发布/protected 产物不修剪）。下方列出已发布（protected）路径。
      </p>
      <pre className="block muted">{policy}</pre>
      <pre className="block muted" style={{ marginTop: '0.5rem' }}>
        {published}
      </pre>
    </section>
  );
}
