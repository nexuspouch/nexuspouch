import { useState } from 'react';
import { api } from '../../shared/api';
import { fmtBytes } from '../../shared/format';
import { useFeedback } from '../../shared/ui/feedback';
import type { Stats } from '../types';

type Props = {
  stats: Stats | null;
  recycleCount: number;
  onError: (e: unknown) => void;
  onRefresh: () => Promise<void>;
  onGoBrowse: () => void;
  onGoRecycle: () => void;
  onDismiss?: () => void;
};

type Step = 'intro' | 'gc' | 'recycle' | 'done';

export function CleanupWizard({
  stats,
  recycleCount,
  onError,
  onRefresh,
  onGoBrowse,
  onGoRecycle,
  onDismiss,
}: Props) {
  const { toast, confirm } = useFeedback();
  const [step, setStep] = useState<Step>('intro');
  const [busy, setBusy] = useState(false);
  const [gcResult, setGcResult] = useState('');
  const [emptyResult, setEmptyResult] = useState('');

  const usedPct = Math.round((Number(stats?.volume_used_ratio) || 0) * 100);

  async function runGc() {
    setBusy(true);
    try {
      const out = await api<{ staging_removed?: number; recycle_bytes?: number }>(
        '/admin/api/gc',
        { method: 'POST' },
      );
      const msg = `staging 清理 ${out.staging_removed || 0} · 回收站释放 ${fmtBytes(
        Number(out.recycle_bytes),
      )}`;
      setGcResult(msg);
      toast(`GC 完成：${msg}`, { kind: 'ok' });
      await onRefresh();
      setStep('recycle');
    } catch (e) {
      onError(e);
    } finally {
      setBusy(false);
    }
  }

  async function emptyRecycle() {
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
      setEmptyResult('回收站已清空');
      toast('回收站已清空', { kind: 'ok' });
      await onRefresh();
      setStep('done');
    } catch (e) {
      onError(e);
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="panel cleanup-wizard" id="cleanup">
      <div className="section-head">
        <h2>磁盘清理向导</h2>
        {onDismiss ? (
          <button type="button" className="ghost" onClick={onDismiss}>
            关闭
          </button>
        ) : null}
      </div>

      <ol className="wizard-steps" aria-label="清理步骤">
        {(
          [
            ['intro', '说明'],
            ['gc', 'GC'],
            ['recycle', '回收站'],
            ['done', '完成'],
          ] as const
        ).map(([id, label], i) => {
          const order: Step[] = ['intro', 'gc', 'recycle', 'done'];
          const active = order.indexOf(step);
          return (
            <li
              key={id}
              className={
                i < active ? 'done' : i === active ? 'current' : undefined
              }
            >
              <span className="wizard-num">{i + 1}</span>
              {label}
            </li>
          );
        })}
      </ol>

      {step === 'intro' ? (
        <div className="wizard-body">
          <p className="muted" style={{ marginTop: 0 }}>
            卷已用约 <strong>{usedPct}%</strong>
            {stats?.volume_free_bytes != null
              ? `（剩余 ${fmtBytes(Number(stats.volume_free_bytes))}）`
              : ''}
            。按顺序：先 GC staging/过期回收，再处理回收站或手删大文件。
          </p>
          <div className="row">
            <button
              type="button"
              className="primary"
              disabled={busy}
              onClick={() => setStep('gc')}
            >
              开始清理
            </button>
            <button type="button" onClick={onGoBrowse}>
              先去浏览
            </button>
          </div>
        </div>
      ) : null}

      {step === 'gc' ? (
        <div className="wizard-body">
          <p className="muted" style={{ marginTop: 0 }}>
            清理 staging 暂存与可回收空间。安全、可重复执行。
          </p>
          {gcResult ? <p className="ok">{gcResult}</p> : null}
          <div className="row">
            <button
              type="button"
              className="primary"
              disabled={busy}
              onClick={() => void runGc()}
            >
              {busy ? '执行中…' : '运行 GC'}
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={() => setStep('recycle')}
            >
              跳过
            </button>
          </div>
        </div>
      ) : null}

      {step === 'recycle' ? (
        <div className="wizard-body">
          <p className="muted" style={{ marginTop: 0 }}>
            回收站当前 {recycleCount} 项。可清空释放空间，或去回收站逐项还原。
          </p>
          {emptyResult ? <p className="ok">{emptyResult}</p> : null}
          <div className="row">
            <button
              type="button"
              className="danger"
              disabled={busy || recycleCount === 0}
              onClick={() => void emptyRecycle()}
            >
              清空回收站
            </button>
            <button type="button" onClick={onGoRecycle}>
              打开回收站
            </button>
            <button
              type="button"
              className="primary"
              onClick={() => setStep('done')}
            >
              下一步
            </button>
          </div>
        </div>
      ) : null}

      {step === 'done' ? (
        <div className="wizard-body">
          <p className="ok" style={{ marginTop: 0 }}>
            清理步骤完成。若空间仍紧张，可浏览分区删除大文件，或扩大磁盘。
          </p>
          <div className="row">
            <button type="button" className="primary" onClick={onGoBrowse}>
              去浏览删文件
            </button>
            {onDismiss ? (
              <button type="button" onClick={onDismiss}>
                完成
              </button>
            ) : null}
          </div>
        </div>
      ) : null}
    </section>
  );
}
