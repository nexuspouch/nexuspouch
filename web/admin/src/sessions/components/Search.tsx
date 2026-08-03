import { FormEvent, useEffect, useMemo, useState } from 'react';
import { api, postFeedback } from '../../shared/api';
import { escHtml, shortId } from '../../shared/format';
import { useFeedback } from '../../shared/ui/feedback';
import type { SearchHit } from '../types';

function countAdvanced(params: {
  agent: string;
  project: string;
  since: string;
  until: string;
  device: string;
  dedup: boolean;
}): number {
  let n = 0;
  if (params.agent.trim()) n += 1;
  if (params.project.trim()) n += 1;
  if (params.since) n += 1;
  if (params.until) n += 1;
  if (params.device) n += 1;
  if (!params.dedup) n += 1;
  return n;
}

export function SearchPage({
  params,
  onNotice,
  onError,
}: {
  params: URLSearchParams;
  onNotice: (msg: string) => void;
  onError: (err: unknown) => void;
}) {
  const { toast } = useFeedback();
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
  const [advancedOpen, setAdvancedOpen] = useState(() => {
    return Boolean(
      params.get('agent') ||
        params.get('project') ||
        params.get('device') ||
        params.get('since_ms') ||
        params.get('until_ms') ||
        params.get('dedup') === 'false',
    );
  });

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
    if (
      params.get('agent') ||
      params.get('project') ||
      params.get('device') ||
      params.get('since_ms') ||
      params.get('until_ms') ||
      params.get('dedup') === 'false'
    ) {
      setAdvancedOpen(true);
    }
  }, [params]);

  useEffect(() => {
    void api<{ devices?: string[] }>('/admin/api/sessions/overview')
      .then((ov) => setDevices(ov.devices || []))
      .catch(() => {});
  }, []);

  const query = (params.get('q') || '').trim();
  const advancedCount = useMemo(
    () => countAdvanced({ agent, project, since, until, device, dedup }),
    [agent, project, since, until, device, dedup],
  );

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
          const msg =
            '向量检索不可用，已回退关键词匹配（需要本地 embedding 模型）。';
          onNotice(msg);
          toast(msg, { kind: 'info', durationMs: 5000 });
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
  }, [query, params, onError, onNotice, toast]);

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

  function clearAdvanced() {
    setAgent('');
    setProject('');
    setSince('');
    setUntil('');
    setDevice('');
    setDedup(true);
  }

  return (
    <div className="panel search-panel">
      <div className="section-head" style={{ marginBottom: 12 }}>
        <h2 style={{ margin: 0, fontSize: '1rem', fontWeight: 650 }}>
          搜索会话
        </h2>
        {query ? <span className="hint">{meta || (loading ? '搜索中…' : '')}</span> : null}
      </div>

      <form className="search-form" onSubmit={submit}>
        <div className="search-primary">
          <input
            className="search-q"
            placeholder="关键词或语义描述…"
            value={q}
            onChange={(e) => setQ(e.target.value)}
            autoFocus
          />
          <label className="search-check">
            <input
              type="checkbox"
              checked={semantic}
              onChange={(e) => setSemantic(e.target.checked)}
            />
            语义回忆
          </label>
          <button type="submit" className="primary" disabled={loading}>
            {loading ? '搜索中…' : '搜索'}
          </button>
        </div>

        <div className="search-advanced-toggle">
          <button
            type="button"
            className="ghost"
            onClick={() => setAdvancedOpen((v) => !v)}
            aria-expanded={advancedOpen}
          >
            {advancedOpen ? '收起筛选' : '高级筛选'}
            {advancedCount > 0 ? (
              <span className="nav-count">{advancedCount}</span>
            ) : null}
          </button>
          {advancedCount > 0 ? (
            <button type="button" className="ghost" onClick={clearAdvanced}>
              清除筛选
            </button>
          ) : null}
        </div>

        {advancedOpen ? (
          <div className="search-advanced">
            <label>
              <span>Agent</span>
              <input
                placeholder="claude / codex…"
                value={agent}
                onChange={(e) => setAgent(e.target.value)}
              />
            </label>
            <label>
              <span>Project</span>
              <input
                placeholder="项目名"
                value={project}
                onChange={(e) => setProject(e.target.value)}
              />
            </label>
            <label>
              <span>起始</span>
              <input
                type="date"
                value={since}
                onChange={(e) => setSince(e.target.value)}
              />
            </label>
            <label>
              <span>结束</span>
              <input
                type="date"
                value={until}
                onChange={(e) => setUntil(e.target.value)}
              />
            </label>
            <label>
              <span>设备</span>
              <select
                value={device}
                onChange={(e) => setDevice(e.target.value)}
              >
                <option value="">全部设备</option>
                {devices.map((d) => (
                  <option key={d} value={d}>
                    {shortId(d)}
                  </option>
                ))}
              </select>
            </label>
            <label className="search-check advanced-check">
              <input
                type="checkbox"
                checked={dedup}
                onChange={(e) => setDedup(e.target.checked)}
              />
              按项目去重
            </label>
          </div>
        ) : null}
      </form>

      {!query && (
        <p className="hint" style={{ marginTop: 14 }}>
          输入查询开始。默认关键词匹配；勾选「语义回忆」走 FTS5+向量混合。
        </p>
      )}

      {loading && query ? (
        <div className="search-loading" aria-live="polite">
          正在搜索…
        </div>
      ) : null}

      <div className="search-hits">
        {hits.map((h, i) => (
          <div className="hit" key={`${h.uri}-${i}`}>
            <div className="hit-head">
              <a
                className="title-link"
                href={`#/s/${encodeURIComponent(h.uri)}`}
                onClick={() => {
                  void postFeedback(query, h, i + 1, 'click').catch(() => {});
                }}
              >
                {h.path || h.uri}
              </a>
              <span className="hint">设备 {shortId(h.device || '')}</span>
              {h.score_type ? (
                <span className="badge accent">{h.score_type}</span>
              ) : null}
              {h.occurrence_label ? (
                <span className="badge">{h.occurrence_label}</span>
              ) : null}
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
              <pre className="snip fragment-preview">
                {h.fragment.turns
                  .map(
                    (t) =>
                      `${t.focus ? '▶ ' : '  '}${t.role || ''}: ${String(
                        t.content || '',
                      ).slice(0, 160)}`,
                  )
                  .join('\n')}
              </pre>
            )}
            <div className="actions">
              <a
                className="btn-link"
                href={`#/?uri=${encodeURIComponent(h.uri)}`}
              >
                在总览打开
              </a>
              <a
                className="btn-link"
                href={`#/s/${encodeURIComponent(h.uri)}`}
              >
                全屏详情
              </a>
              <button
                type="button"
                onClick={async () => {
                  try {
                    await postFeedback(query, h, i + 1, 'adopt');
                    onNotice('已记录：有用');
                    toast('已记录：有用', { kind: 'ok' });
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
                    toast('已记录：结果不对', { kind: 'info' });
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
      </div>

      {query && !loading && !hits.length ? (
        <div className="empty">
          没有命中。换个说法试试，或先{' '}
          <a href="#/bind">绑定会话目录</a> 确认已入库。
        </div>
      ) : null}
    </div>
  );
}
