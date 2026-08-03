import { api } from '../../shared/api';
import { fmtBytes } from '../../shared/format';
import type { RecycleEntry } from '../types';

type Props = {
  entries: RecycleEntry[];
  onError: (e: unknown) => void;
  onRefresh: () => Promise<void>;
};

export function RecyclePanel({ entries, onError, onRefresh }: Props) {
  async function empty() {
    if (!confirm('确认清空回收站？不可还原。')) return;
    try {
      await api('/admin/api/recycle/empty', { method: 'POST' });
      await onRefresh();
    } catch (e) {
      onError(e);
    }
  }

  async function restore(recyclePath: string) {
    try {
      await api('/admin/api/recycle/restore', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ recycle_path: recyclePath }),
      });
      await onRefresh();
    } catch (e) {
      onError(e);
    }
  }

  return (
    <section className="panel">
      <div className="section-head">
        <h2>回收站</h2>
        <button type="button" className="danger" onClick={() => void empty()}>
          清空回收站
        </button>
      </div>
      {!entries.length ? (
        <p className="muted">回收站为空</p>
      ) : (
        <table className="data">
          <thead>
            <tr>
              <th>路径</th>
              <th>大小</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {entries.map((e) => {
              const path = `${e.space || ''}/${e.origin_path || ''}`;
              return (
                <tr key={e.recycle_path || path}>
                  <td>
                    {path}
                    <div className="muted">{e.recycle_path || ''}</div>
                  </td>
                  <td>{fmtBytes(Number(e.size))}</td>
                  <td>
                    <button
                      type="button"
                      onClick={() => void restore(e.recycle_path || '')}
                    >
                      还原
                    </button>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </section>
  );
}
