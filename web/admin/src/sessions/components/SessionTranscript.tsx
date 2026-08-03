import { useEffect, useState } from 'react';
import { api } from '../../shared/api';
import {
  fmtBytes,
  fmtTime,
  shortId,
  toolName,
} from '../../shared/format';
import type { SessionDetail } from '../types';

export function SessionTranscript({
  uri,
  onError,
}: {
  uri: string;
  onNotice?: (msg: string) => void;
  onError: (err: unknown) => void;
}) {
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [refUri, setRefUri] = useState(uri);

  useEffect(() => {
    setRefUri(uri);
  }, [uri]);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      setLoading(true);
      onError('');
      try {
        const d = await api<SessionDetail>(
          `/admin/api/sessions/detail?uri=${encodeURIComponent(refUri)}`,
        );
        if (!cancelled) setDetail(d);
      } catch (e) {
        if (!cancelled) {
          setDetail(null);
          onError(e);
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [refUri, onError]);

  if (loading) return <p className="hint">加载中…</p>;
  if (!detail) return <p className="err">无法加载会话</p>;

  const versions = detail.versions || [];
  const top = versions[versions.length - 1];

  return (
    <div>
      <div
        style={{
          display: 'flex',
          flexWrap: 'wrap',
          gap: 12,
          justifyContent: 'space-between',
          marginBottom: 12,
        }}
      >
        <div>
          <h2 style={{ margin: 0, fontSize: '1.05rem' }}>
            {detail.agent || ''} · {detail.session_id || ''}
          </h2>
          <p className="hint" style={{ margin: '6px 0 0' }}>
            {detail.project ? `${detail.project} · ` : ''}
            设备 {shortId(detail.device || '')} · {detail.kind || ''} ·{' '}
            {fmtBytes(detail.size || 0)}
            <span className="badge" style={{ marginLeft: 8 }}>
              原文未脱敏
            </span>
            {detail.protected && (
              <span className="badge accent" style={{ marginLeft: 6 }}>
                protected
              </span>
            )}
          </p>
        </div>
        <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
          {versions.length > 0 && (
            <>
              <span className="hint">版本</span>
              <select
                value={
                  detail.ref === 'latest'
                    ? `v${detail.version != null ? detail.version : top.v}`
                    : detail.ref || ''
                }
                onChange={(e) => {
                  const pick = e.target.value;
                  const next =
                    top && `v${top.v}` === pick
                      ? detail.uri
                      : `${detail.uri}@${pick}`;
                  setRefUri(next);
                }}
              >
                {versions.map((v) => (
                  <option key={v.v} value={`v${v.v}`}>
                    v{v.v} · {fmtTime(v.mtime)} · {fmtBytes(v.size)}
                  </option>
                ))}
              </select>
            </>
          )}
          <a href={`#/s/${encodeURIComponent(detail.uri)}`}>全屏</a>
        </div>
      </div>

      {detail.truncated && (
        <p className="hint">（内容过长，已截断显示）</p>
      )}

      {(detail.turns || []).map((t, i) =>
        t.role === 'tool' ? (
          <details className="tool" key={i}>
            <summary>🔧 {toolName(t.content || '')}</summary>
            <pre>{t.content || ''}</pre>
          </details>
        ) : (
          <div
            className={`turn ${t.role === 'user' ? 'user' : 'assistant'}`}
            key={i}
          >
            <div className="meta">
              {t.role === 'user' ? '用户' : '助手'}
              {t.ts_ms ? ` · ${fmtTime(t.ts_ms)}` : ''}
            </div>
            <div>{t.content || ''}</div>
          </div>
        ),
      )}

      {!(detail.turns || []).length && (
        <p className="hint">无法解析出对话内容——文件可能为空或格式未识别</p>
      )}
    </div>
  );
}

/** Full-page detail route (#/s/...) */
export function DetailPage({
  uri,
  onError,
}: {
  uri: string;
  onError: (err: unknown) => void;
}) {
  return (
    <div className="panel">
      <p style={{ marginTop: 0 }}>
        <a href="#/">← 返回总览</a>
      </p>
      <SessionTranscript uri={uri} onError={onError} />
    </div>
  );
}
