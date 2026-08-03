import { FormEvent, useEffect, useState } from 'react';
import { api, postFeedback } from '../../shared/api';
import { escHtml, shortId } from '../../shared/format';
import type { SearchHit } from '../types';

export function SearchPage({
  params,
  onNotice,
  onError,
}: {
  params: URLSearchParams;
  onNotice: (msg: string) => void;
  onError: (err: unknown) => void;
}) {
  const [q, setQ] = useState(params.get('q') || '');
  const [agent, setAgent] = useState(params.get('agent') || '');
  const [project, setProject] = useState(params.get('project') || '');
  const [since, setSince] = useState('');
  const [until, setUntil] = useState('');
  const [device, setDevice] = useState(params.get('device') || '');
  const [semantic, setSemantic] = useState(params.get('semantic') === 'true');
  const [dedup, setDedup] = useState(params.get('dedup') !== 'false');
  const [devices, setDevices] = useState<string[]>([]);
  const [loading, setLoading] = useState(false);
  const [meta, setMeta] = useState('');
  const [hits, setHits] = useState<SearchHit[]>([]);

  useEffect(() => {
    setQ(params.get('q') || '');
    setAgent(params.get('agent') || '');
    setProject(params.get('project') || '');
    setDevice(params.get('device') || '');
    setSemantic(params.get('semantic') === 'true');
    setDedup(params.get('dedup') !== 'false');
    if (params.get('since_ms')) {
      const d = new Date(Number(params.get('since_ms')));
      if (!Number.isNaN(d.getTime())) setSince(d.toISOString().slice(0, 10));
    } else setSince('');
    if (params.get('until_ms')) {
      const d = new Date(Number(params.get('until_ms')));
      if (!Number.isNaN(d.getTime())) setUntil(d.toISOString().slice(0, 10));
    } else setUntil('');
  }, [params]);

  useEffect(() => {
    void api<{ devices?: string[] }>('/admin/api/sessions/overview')
      .then((ov) => setDevices(ov.devices || []))
      .catch(() => {});
  }, []);

  const query = (params.get('q') || '').trim();

  useEffect(() => {
    if (!query) {
      setHits([]);
      setMeta('');
      return;
    }
    let cancelled = false;
    (async () => {
      setLoading(true);
      onError('');
      try {
        const p = new URLSearchParams({ q: query });
        if (params.get('semantic') === 'true') p.set('semantic', 'true');
        if (params.get('dedup') === 'false') p.set('dedup', 'false');
        for (const k of ['device', 'agent', 'project', 'since_ms', 'until_ms']) {
          const v = params.get(k);
          if (v) p.set(k, v);
        }
        const out = await api<{
          total?: number;
          score_type?: string;
          rerank?: boolean;
          dedup?: boolean;
          embedder?: string;
          degraded?: boolean;
          results?: SearchHit[];
        }>(`/admin/api/sessions/search?${p}`);
        if (cancelled) return;
        setMeta(
          `命中 ${out.total || 0} · ${out.score_type || 'keyword'}` +
            (out.rerank ? ' · rerank' : '') +
            (out.dedup ? ' · dedup' : '') +
            (out.embedder ? ` · ${out.embedder}` : ''),
        );
        if (out.degraded) {
          onNotice(
            '向量检索不可用，已回退关键词匹配（需要本地 embedding 模型）。',
          );
        }
        setHits(out.results || []);
      } catch (e) {
        if (!cancelled) onError(e);
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [query, params, onError, onNotice]);

  const submit = (ev: FormEvent) => {
    ev.preventDefault();
    const p = new URLSearchParams();
    if (q.trim()) p.set('q', q.trim());
    if (agent.trim()) p.set('agent', agent.trim());
    if (project.trim()) p.set('project', project.trim());
    if (since) p.set('since_ms', String(Date.parse(`${since}T00:00:00Z`)));
    if (until) p.set('until_ms', String(Date.parse(`${until}T23:59:59Z`)));
    if (semantic) p.set('semantic', 'true');
    if (!dedup) p.set('dedup', 'false');
    if (device) p.set('device', device);
    location.hash = `#/search?${p}`;
  };

  return (
    <div className="panel">
      <h2 style={{ marginTop: 0 }}>搜索会话</h2>
      <form className="form-row" onSubmit={submit} style={{ marginBottom: 14 }}>
        <input
          placeholder="关键词或语义描述…"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          style={{ minWidth: '16rem', flex: 1 }}
        />
        <input
          placeholder="agent"
          value={agent}
          onChange={(e) => setAgent(e.target.value)}
          style={{ width: '7.5rem' }}
        />
        <input
          placeholder="project"
          value={project}
          onChange={(e) => setProject(e.target.value)}
          style={{ width: '7.5rem' }}
        />
        <input type="date" value={since} onChange={(e) => setSince(e.target.value)} />
        <input type="date" value={until} onChange={(e) => setUntil(e.target.value)} />
        <select value={device} onChange={(e) => setDevice(e.target.value)}>
          <option value="">全部设备</option>
          {devices.map((d) => (
            <option key={d} value={d}>
              {shortId(d)}
            </option>
          ))}
        </select>
        <label className="hint" style={{ display: 'flex', gap: 6, alignItems: 'center' }}>
          <input
            type="checkbox"
            checked={semantic}
            onChange={(e) => setSemantic(e.target.checked)}
          />
          语义回忆
        </label>
        <label className="hint" style={{ display: 'flex', gap: 6, alignItems: 'center' }}>
          <input
            type="checkbox"
            checked={dedup}
            onChange={(e) => setDedup(e.target.checked)}
          />
          按项目去重
        </label>
        <button type="submit" className="primary" disabled={loading}>
          {loading ? '搜索中…' : '搜索'}
        </button>
      </form>

      {!query && (
        <p className="hint">
          输入查询开始。可按 agent / project / 日期过滤；语义回忆走 FTS5+向量混合。
        </p>
      )}
      {query && <p className="hint">{meta || (loading ? '搜索中…' : '')}</p>}

      {hits.map((h, i) => (
        <div className="hit" key={`${h.uri}-${i}`}>
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8, alignItems: 'center' }}>
            <a
              className="title-link"
              href={`#/s/${encodeURIComponent(h.uri)}`}
              style={{ fontWeight: 650 }}
              onClick={() => {
                void postFeedback(query, h, i + 1, 'click').catch(() => {});
              }}
            >
              {h.path || h.uri}
            </a>
            <span className="hint">设备 {shortId(h.device || '')}</span>
            {h.score_type && <span className="badge accent">{h.score_type}</span>}
            {h.occurrence_label && (
              <span className="badge">{h.occurrence_label}</span>
            )}
          </div>
          <div
            className="snip"
            dangerouslySetInnerHTML={{
              __html: escHtml(h.snippet || '')
                .replaceAll('&lt;b&gt;', '<b>')
                .replaceAll('&lt;/b&gt;', '</b>'),
            }}
          />
          {!!h.fragment?.turns?.length && (
            <pre
              className="snip"
              style={{ opacity: 0.9, whiteSpace: 'pre-wrap', marginTop: 8 }}
            >
              {h.fragment.turns
                .map(
                  (t) =>
                    `${t.focus ? '▶ ' : '  '}${t.role || ''}: ${String(t.content || '').slice(0, 160)}`,
                )
                .join('\n')}
            </pre>
          )}
          <div className="actions">
            <button
              type="button"
              onClick={async () => {
                try {
                  await postFeedback(query, h, i + 1, 'adopt');
                  onNotice('已记录：有用');
                } catch (e) {
                  onError(e);
                }
              }}
            >
              有用
            </button>
            <button
              type="button"
              className="danger"
              onClick={async () => {
                try {
                  await postFeedback(query, h, i + 1, 'wrong');
                  onNotice('已记录：结果不对');
                } catch (e) {
                  onError(e);
                }
              }}
            >
              不对
            </button>
          </div>
        </div>
      ))}

      {query && !loading && !hits.length && (
        <div className="empty">没有命中。换个说法试试，或先确认会话已入库。</div>
      )}
    </div>
  );
}
