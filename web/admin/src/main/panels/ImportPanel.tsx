import { api } from '../../shared/api';
import { shortId } from '../../shared/format';
import type { ImportRequest } from '../types';

type Props = {
  requests: ImportRequest[];
  issued: string;
  received: string;
  onError: (e: unknown) => void;
  onRefresh: () => Promise<void>;
  onNotice: (msg: string) => void;
};

export function ImportPanel({
  requests,
  issued,
  received,
  onError,
  onRefresh,
  onNotice,
}: Props) {
  async function grant(id: string) {
    try {
      const out = await api<{ pushed?: boolean }>('/admin/api/import/grant', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ request_id: id }),
      });
      if (!out.pushed) {
        onNotice(
          '已签发；请求方当前不在线，待其重连后自行拉取/再授权推送。',
        );
      } else {
        onNotice('');
      }
      await onRefresh();
    } catch (e) {
      onError(e);
    }
  }

  async function reject(id: string) {
    try {
      await api('/admin/api/import/reject', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ request_id: id }),
      });
      await onRefresh();
    } catch (e) {
      onError(e);
    }
  }

  return (
    <section className="panel">
      <div className="section-head">
        <h2>换机导入请求</h2>
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
                  <button type="button" className="primary" onClick={() => void grant(r.request_id)}>
                    批准
                  </button>
                  <button type="button" onClick={() => void reject(r.request_id)}>
                    拒绝
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <h3 className="subhead">已签发授权</h3>
      <pre className="block muted">{issued}</pre>
      <h3 className="subhead">已获授权</h3>
      <p className="muted">本节点作为新设备收到的推送（路径 A/B；对齐 App import_received）。</p>
      <pre className="block muted">{received}</pre>
    </section>
  );
}
