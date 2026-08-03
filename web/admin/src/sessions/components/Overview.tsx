import { useEffect, useMemo, useState } from 'react';
import { api } from '../../shared/api';
import {
  fmtBytes,
  fmtDuration,
  fmtRelative,
  shortId,
} from '../../shared/format';
import type { SessionRow } from '../types';
import { SessionTranscript } from './SessionTranscript';

const PAGE_SIZE = 100;

export function OverviewPage({
  params,
  onNotice,
  onError,
}: {
  params: URLSearchParams;
  onNotice: (msg: string) => void;
  onError: (err: unknown) => void;
}) {
  const initialUri = params.get('uri') || '';
  const [device, setDevice] = useState(params.get('device') || '');
  const [filter, setFilter] = useState('');
  const [agentFilter, setAgentFilter] = useState('');
  const [devices, setDevices] = useState<string[]>([]);
  const [sessions, setSessions] = useState<SessionRow[]>([]);
  const [total, setTotal] = useState(0);
  const [hasMore, setHasMore] = useState(false);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [selected, setSelected] = useState<string | null>(initialUri || null);

  useEffect(() => {
    const uri = params.get('uri') || '';
    if (uri) setSelected(uri);
    const d = params.get('device') || '';
    if (d) setDevice(d);
  }, [params]);

  const load = async (dev?: string, offset = 0, append = false) => {
    if (append) setLoadingMore(true);
    else setLoading(true);
    onError('');
    try {
      const q = new URLSearchParams();
      if (dev) q.set('device', dev);
      q.set('limit', String(PAGE_SIZE));
      q.set('offset', String(offset));
      const out = await api<{
        devices?: string[];
        total?: number;
        truncated?: boolean;
        has_more?: boolean;
        sessions?: SessionRow[];
      }>(`/admin/api/sessions/overview?${q}`);
      setDevices(out.devices || []);
      const list = out.sessions || [];
      setSessions((prev) => {
        const next = append ? [...prev, ...list] : list;
        setSelected((sel) => {
          if (sel && next.some((s) => s.uri === sel)) return sel;
          const want = params.get('uri');
          if (want && next.some((s) => s.uri === want)) return want;
          return null;
        });
        return next;
      });
      setTotal(out.total || 0);
      setHasMore(!!out.has_more);
    } catch (e) {
      onError(e);
    } finally {
      setLoading(false);
      setLoadingMore(false);
    }
  };

  useEffect(() => {
    void load(device, 0, false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [device]);

  const agents = useMemo(() => {
    const set = new Set<string>();
    for (const s of sessions) {
      if (s.agent) set.add(s.agent);
    }
    return [...set].sort();
  }, [sessions]);

  const filtered = useMemo(() => {
    const f = filter.trim().toLowerCase();
    return sessions.filter((s) => {
      if (agentFilter && (s.agent || '') !== agentFilter) return false;
      if (!f) return true;
      const hay = [
        s.title,
        s.agent,
        s.project,
        s.session_id,
        s.path,
        s.kind,
      ]
        .filter(Boolean)
        .join(' ')
        .toLowerCase();
      return hay.includes(f);
    });
  }, [sessions, filter, agentFilter]);

  const groups = useMemo(() => {
    const map: Record<string, SessionRow[]> = {};
    const order: string[] = [];
    for (const s of filtered) {
      const a = s.agent || 'unknown';
      if (!map[a]) {
        map[a] = [];
        order.push(a);
      }
      map[a].push(s);
    }
    return order.map((a) => ({ agent: a, items: map[a] }));
  }, [filtered]);

  function selectSession(uri: string) {
    setSelected(uri);
    const p = new URLSearchParams();
    p.set('uri', uri);
    if (device) p.set('device', device);
    const next = `#/?${p}`;
    if (location.hash !== next) {
      history.replaceState(null, '', next);
    }
  }

  return (
    <div className="panel overview-panel">
      <div className="toolbar overview-toolbar">
        <div className="left">
          <input
            placeholder="筛选标题 / agent / project…"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            className="overview-filter"
          />
          <select
            value={device}
            onChange={(e) => setDevice(e.target.value)}
            aria-label="设备"
          >
            <option value="">全部设备</option>
            {devices.map((d) => (
              <option key={d} value={d}>
                {shortId(d)}
              </option>
            ))}
          </select>
          <span className="hint">
            已加载 {sessions.length}/{total}
            {hasMore ? '（还有更多）' : ''}
          </span>
        </div>
        <div className="right">
          {(filter || agentFilter) && (
            <button
              type="button"
              className="ghost"
              onClick={() => {
                setFilter('');
                setAgentFilter('');
              }}
            >
              清除
            </button>
          )}
          <a className="btn-link" href="#/search">
            去搜索
          </a>
          <button
            type="button"
            onClick={() => void load(device, 0, false)}
            disabled={loading}
          >
            {loading ? '加载中…' : '刷新'}
          </button>
        </div>
      </div>

      {hasMore ? (
        <div className="banner show truncate-banner" role="status">
          已显示 {sessions.length} / {total}。可继续加载，或用{' '}
          <a href="#/search">搜索</a> 精确定位。
        </div>
      ) : null}

      {agents.length > 1 ? (
        <div className="agent-chips" role="list">
          <button
            type="button"
            className={!agentFilter ? 'chip active' : 'chip'}
            onClick={() => setAgentFilter('')}
          >
            全部
          </button>
          {agents.map((a) => (
            <button
              key={a}
              type="button"
              className={agentFilter === a ? 'chip active' : 'chip'}
              onClick={() => setAgentFilter(agentFilter === a ? '' : a)}
            >
              {a}
            </button>
          ))}
        </div>
      ) : null}

      {!loading && !sessions.length ? (
        <div className="empty">
          暂无会话。请先 <a href="#/bind">绑定 agent 会话目录</a>
          ，同步后会出现在这里。
        </div>
      ) : (
        <div className="split">
          <div className="list-pane">
            {groups.map((g) => (
              <div key={g.agent}>
                <div className="agent-group">
                  {g.agent} · {g.items.length}
                </div>
                {g.items.map((s) => (
                  <button
                    key={s.uri}
                    type="button"
                    className={
                      'session-item' + (selected === s.uri ? ' selected' : '')
                    }
                    onClick={() => selectSession(s.uri)}
                  >
                    <div className="title">
                      {s.title || s.session_id || s.uri}
                    </div>
                    <div className="meta">
                      <span>{s.project || '—'}</span>
                      <span>{fmtRelative(s.mtime || 0)}</span>
                      <span>{fmtDuration(s.duration_ms)}</span>
                      <span>{fmtBytes(s.size || 0)}</span>
                    </div>
                  </button>
                ))}
              </div>
            ))}
            {loading && <div className="empty">加载中…</div>}
            {!loading && !filtered.length && (
              <div className="empty">没有匹配的会话</div>
            )}
            {hasMore ? (
              <div className="load-more">
                <button
                  type="button"
                  disabled={loadingMore}
                  onClick={() => void load(device, sessions.length, true)}
                >
                  {loadingMore ? '加载中…' : '加载更多'}
                </button>
              </div>
            ) : null}
          </div>
          <div className="detail-pane">
            {selected ? (
              <SessionTranscript
                uri={selected}
                onNotice={onNotice}
                onError={onError}
              />
            ) : (
              <div className="empty">从左侧选择一个会话查看对话</div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
