import { useState } from 'react';
import { api } from '../../shared/api';
import { fmtBytes } from '../../shared/format';

type Props = {
  onError: (e: unknown) => void;
  onRefresh: () => Promise<void>;
};

export function DangerPanel({ onError, onRefresh }: Props) {
  const [confirmText, setConfirmText] = useState('');

  async function wipeSelf() {
    if (confirmText.trim() !== 'DELETE') {
      onError(new Error('请先在输入框输入 DELETE'));
      return;
    }
    if (!confirm('确认清空本机 store？此操作不可从回收站还原。')) return;
    try {
      const out = await api<{ freed_bytes?: number }>(
        '/admin/api/devices/wipe-self',
        {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ confirm: 'DELETE' }),
        },
      );
      setConfirmText('');
      alert(`已清空本机 store，释放 ${fmtBytes(Number(out.freed_bytes))}`);
      await onRefresh();
    } catch (e) {
      onError(e);
    }
  }

  return (
    <section className="panel">
      <h2>危险区</h2>
      <p className="muted">
        清空本机四分区正式文件与暂存（不删他端镜像、回收站、.system）。不可从回收站还原。
      </p>
      <div className="row">
        <input
          placeholder="输入 DELETE 确认"
          style={{ minWidth: '12rem' }}
          value={confirmText}
          onChange={(e) => setConfirmText(e.target.value)}
        />
        <button type="button" className="danger" onClick={() => void wipeSelf()}>
          清空本机 store
        </button>
      </div>
    </section>
  );
}
