import { useState } from 'react';
import { api } from '../../shared/api';
import { fmtRelative, shortId } from '../../shared/format';
import { useFeedback } from '../../shared/ui/feedback';
import type { ImportGrant, ImportRequest } from '../types';

type Props = {
  requests: ImportRequest[];
  issued: ImportGrant[];
  received: ImportGrant[];
  onError: (e: unknown) => void;
  onRefresh: () => Promise<void>;
  onNotice: (msg: string) => void;
};

function GrantTable({
  rows,
  empty,
}: {
  rows: ImportGrant[];
  empty: string;
}) {
  if (!rows.length) return <p className="muted">{empty}</p>;
  return (
    <table className="data">
      <thead>
        <tr>
          <th>授权</th>
          <th>设备</th>
          <th>空间</th>
          <th>时效</th>
          <th>状态</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((g) => {
          const expired =
            Number(g.expires_at) > 0 && Number(g.expires_at) < Date.now();
          const status = g.revoked
            ? '已吊销'
            : expired
              ? '已过期'
              : '有效';
          return (
            <tr key={g.grant_id || `${g.old_device}-${g.new_device}-${g.issued_at}`}>
              <td className="mono">{shortId(g.grant_id || '')}</td>
              <td>
                <div>
                  旧 {shortId(g.old_device || '')} → 新{' '}
                  {shortId(g.new_device || '')}
                </div>
                <div className="muted">{fmtRelative(Number(g.issued_at))}</div>
              </td>
              <td>
                <div className="chip-row">
                  {(g.spaces || []).map((s) => (
                    <span key={s} className="badge">
                      {s}
                    </span>
                  ))}
                </div>
              </td>
              <td className="muted">{fmtRelative(Number(g.expires_at))}</td>
              <td>
                <span
                  className={`badge ${
                    status === '有效' ? 'ok' : status === '已吊销' ? '' : ''
                  }`}
                >
                  {status}
                </span>
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}

export function ImportPanel({
  requests,
  issued,
  received,
  onError,
  onRefresh,
  onNotice,
}: Props) {
  const { toast } = useFeedback();
  const [busy, setBusy] = useState(false);

  async function grant(id: string) {
    setBusy(true);
    try {
      const out = await api<{ pushed?: boolean }>('/admin/api/import/grant', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ request_id: id }),
      });
      if (!out.pushed) {
        const msg =
          '已签发；请求方当前不在线，待其重连后自行拉取/再授权推送。';
        onNotice(msg);
        toast(msg, { kind: 'info', durationMs: 6000 });
      } else {
        onNotice('');
        toast('已批准并推送', { kind: 'ok' });
      }
      await onRefresh();
    } catch (e) {
      onError(e);
    } finally {
      setBusy(false);
    }
  }

  async function reject(id: string) {
    setBusy(true);
    try {
      await api('/admin/api/import/reject', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ request_id: id }),
      });
      toast('已拒绝', { kind: 'ok' });
      await onRefresh();
    } catch (e) {
      onError(e);
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="panel">
      <div className="section-head">
        <h2>换机导入请求</h2>
        {requests.length ? (
          <span className="badge accent">{requests.length} 待审</span>
        ) : null}
      </div>
      {!requests.length ? (
        <p className="muted">暂无待审批请求</p>
      ) : (
        <table className="data">
          <thead>
            <tr>
              <th>新设备</th>
              <th>旧设备</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {requests.map((r) => (
              <tr key={r.request_id}>
                <td>{shortId(r.new_device || '')}</td>
                <td>{shortId(r.old_device || '')}</td>
                <td className="row">
                  <button
                    type="button"
                    className="primary"
                    disabled={busy}
                    onClick={() => void grant(r.request_id)}
                  >
                    批准
                  </button>
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => void reject(r.request_id)}
                  >
                    拒绝
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <h3 className="subhead">已签发授权</h3>
      <GrantTable rows={issued} empty="暂无已签发授权" />
      <h3 className="subhead">已获授权</h3>
      <p className="muted">
        本节点作为新设备收到的推送（路径 A/B；对齐 App import_received）。
      </p>
      <GrantTable rows={received} empty="暂无已获授权" />
    </section>
  );
}
