import { useState } from 'react';
import { api } from '../../shared/api';
import { fmtBytes } from '../../shared/format';
import { useFeedback } from '../../shared/ui/feedback';
import type { RecycleEntry } from '../types';

type Props = {
  entries: RecycleEntry[];
  onError: (e: unknown) => void;
  onRefresh: () => Promise<void>;
  onRestored?: () => void;
};

export function RecyclePanel({
  entries,
  onError,
  onRefresh,
  onRestored,
}: Props) {
  const { toast, confirm } = useFeedback();
  const [busy, setBusy] = useState(false);

  async function empty() {
    const ok = await confirm({
      title: '清空回收站',
      message: '确认清空回收站？此操作不可还原。',
      confirmLabel: '清空',
      danger: true,
    });
    if (!ok) return;
    setBusy(true);
    try {
      await api('/admin/api/recycle/empty', { method: 'POST' });
      await onRefresh();
      toast('回收站已清空', { kind: 'ok' });
    } catch (e) {
      onError(e);
    } finally {
      setBusy(false);
    }
  }

  async function restore(recyclePath: string) {
    setBusy(true);
    try {
      await api('/admin/api/recycle/restore', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ recycle_path: recyclePath }),
      });
      await onRefresh();
      toast('已还原', {
        kind: 'ok',
        action: onRestored
          ? { label: '去浏览', onClick: onRestored }
          : undefined,
      });
    } catch (e) {
      onError(e);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="recycle-pane">
      <div className="section-head" style={{ marginTop: 0 }}>
        <p className="muted" style={{ margin: 0 }}>
          从分区浏览删除的文件会出现在这里，可还原或永久清空。
        </p>
        <button
          type="button"
          className="danger"
          disabled={busy || !entries.length}
          onClick={() => void empty()}
        >
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
                      disabled={busy}
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
    </div>
  );
}
