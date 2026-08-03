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

export function OverviewPage({
  onNotice,
  onError,
}: {
  onNotice: (msg: string) => void;
  onError: (err: unknown) => void;
}) {
  const [device, setDevice] = useState('');
  const [filter, setFilter] = useState('');
  const [devices, setDevices] = useState<string[]>([]);
  const [sessions, setSessions] = useState<SessionRow[]>([]);
  const [total, setTotal] = useState(0);
  const [truncated, setTruncated] = useState(false);
  const [loading, setLoading] = useState(true);
  const [selected, setSelected] = useState<string | null>(null);

  const load = async (dev?: string) => {
    setLoading(true);
    onError('');
    try {
      const q = new URLSearchParams();
      if (dev) q.set('device', dev);
      const out = await api<{
        devices?: string[];
        total?: number;
        truncated?: boolean;
        sessions?: SessionRow[];
      }>('/admin/api/sessions/overview' + (q.size ? `?${q}` : ''));
      setDevices(out.devices || []);
      setSessions(out.sessions || []);
      setTotal(out.total || 0);
      setTruncated(!!out.truncated);
      if (
        selected &&
        !(out.sessions || []).some((s) => s.uri === selected)
      ) {
        setSelected(null);
      }
    } catch (e) {
      onError(e);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void load(device);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [device]);

  const filtered = useMemo(() => {
    const f = filter.trim().toLowerCase();
    if (!f) return sessions;
    return sessions.filter((s) => {
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
  }, [sessions, filter]);

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

  return (
    <div className="panel" style={{ padding: 0, overflow: 'hidden' }}>
      <div className="toolbar" style={{ padding: '12px 14px', margin: 0 }}>
        <div className="left">
          <input
            placeholder="筛选标题 / agent / project…"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            style={{ minWidth: '14rem' }}
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
            显示 {filtered.length}/{total}
            {truncated ? '（已截断）' : ''}
          </span>
        </div>
        <div className="right">
          {filter && (
            <button type="button" className="ghost" onClick={() => setFilter('')}>
              清除
            </button>
          )}
          <button
            type="button"
            onClick={() => void load(device)}
            disabled={loading}
          >
            {loading ? '加载中…' : '刷新'}
          </button>
        </div>
      </div>

      {!loading && !sessions.length ? (
        <div className="empty">
          暂无会话。请先{' '}
          <a href="#/bind">绑定 agent 会话目录</a>
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
                    onClick={() => setSelected(s.uri)}
                  >
                    <div className="title">{s.title || s.session_id || s.uri}</div>
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
