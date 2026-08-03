import { useCallback, useEffect, useRef, useState } from 'react';
import { api } from '../../shared/api';
import { shortId } from '../../shared/format';
import { useFeedback } from '../../shared/ui/feedback';
import { StatusPill } from '../../shared/ui/StatusPill';
import type { PairPending, PairStart, Peer } from '../types';

type Props = {
  peers: Peer[];
  selfId: string;
  masterId: string;
  onError: (e: unknown) => void;
  onRefresh: () => Promise<void>;
};

type Step = 'idle' | 'scan' | 'approve' | 'done';

type PairInfo = {
  code?: string;
  local?: string;
  channel?: string;
  fingerprint?: string;
  qrText?: string;
};

export function PairingPanel({
  peers,
  selfId,
  masterId,
  onError,
  onRefresh,
}: Props) {
  const { toast, confirm } = useFeedback();
  const [step, setStep] = useState<Step>('idle');
  const [qrSvg, setQrSvg] = useState('');
  const [info, setInfo] = useState<PairInfo>({});
  const [pending, setPending] = useState<PairPending | null>(null);
  const [doneMsg, setDoneMsg] = useState('');
  const [busy, setBusy] = useState(false);
  const awaiting = useRef(false);
  const timer = useRef<ReturnType<typeof setInterval> | null>(null);
  const prevSid = useRef('');

  const stopPoll = useCallback(() => {
    if (timer.current) {
      clearInterval(timer.current);
      timer.current = null;
    }
  }, []);

  const resetWizard = useCallback(() => {
    awaiting.current = false;
    stopPoll();
    prevSid.current = '';
    setPending(null);
    setQrSvg('');
    setInfo({});
    setStep('idle');
    setDoneMsg('');
  }, [stopPoll]);

  const updatePending = useCallback(async () => {
    try {
      const out = await api<{ pending?: PairPending | null }>(
        '/admin/api/pairing/pending',
      );
      const p = out.pending;
      if (!p) {
        if (prevSid.current) {
          prevSid.current = '';
          setPending(null);
          setQrSvg('');
          setDoneMsg('配对已结束。可再次开始。');
          setStep('done');
          awaiting.current = false;
          stopPoll();
          await onRefresh();
        } else if (!awaiting.current) {
          stopPoll();
        }
        return;
      }
      if (!timer.current) {
        awaiting.current = true;
        timer.current = setInterval(() => {
          void updatePending();
        }, 1000);
      }
      if (prevSid.current === p.session_id) return;
      prevSid.current = p.session_id;
      setPending(p);
      setStep('approve');
    } catch {
      /* ignore */
    }
  }, [onRefresh, stopPoll]);

  useEffect(() => {
    void updatePending();
    return () => stopPoll();
  }, [updatePending, stopPoll]);

  async function startPair() {
    setBusy(true);
    try {
      const out = await api<PairStart>('/admin/api/pairing/start', {
        method: 'POST',
      });
      prevSid.current = '';
      setPending(null);
      setDoneMsg('');
      setQrSvg(out.qr_svg || '');
      setInfo({
        code: out.code,
        local: out.local_endpoint,
        channel: out.channel_endpoint,
        fingerprint: out.fingerprint,
        qrText: out.qr,
      });
      setStep('scan');
      awaiting.current = true;
      stopPoll();
      timer.current = setInterval(() => {
        void updatePending();
      }, 1000);
      await onRefresh();
    } catch (e) {
      onError(e);
    } finally {
      setBusy(false);
    }
  }

  async function decide(accept: boolean) {
    setBusy(true);
    try {
      await api('/admin/api/pairing/decide', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ accept }),
      });
      awaiting.current = false;
      stopPoll();
      prevSid.current = '';
      setPending(null);
      setQrSvg('');
      setInfo({});
      setDoneMsg(accept ? '已批准配对' : '已拒绝配对');
      setStep('done');
      if (accept) toast('配对成功', { kind: 'ok' });
      await onRefresh();
    } catch (e) {
      onError(e);
    } finally {
      setBusy(false);
    }
  }

  async function unpair(fp: string) {
    const ok = await confirm({
      title: '解除配对',
      message: `解除配对 ${shortId(fp)}？`,
      confirmLabel: '解除',
      danger: true,
    });
    if (!ok) return;
    try {
      await api('/admin/api/peers/remove', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ fingerprint: fp }),
      });
      toast(`已解除 ${shortId(fp)}`, { kind: 'ok' });
      await onRefresh();
    } catch (e) {
      onError(e);
    }
  }

  const steps: { id: Step; label: string }[] = [
    { id: 'idle', label: '开始' },
    { id: 'scan', label: '扫码' },
    { id: 'approve', label: '批准' },
    { id: 'done', label: '完成' },
  ];

  function stepIndex(s: Step): number {
    return steps.findIndex((x) => x.id === s);
  }

  const activeIdx = stepIndex(step);

  return (
    <section className="panel">
      <div className="section-head">
        <h2>Noise 配对</h2>
        {step === 'idle' || step === 'done' ? (
          <button
            type="button"
            className="primary"
            disabled={busy}
            onClick={() => void startPair()}
          >
            {step === 'done' ? '再次配对' : '添加设备'}
          </button>
        ) : (
          <button type="button" className="ghost" onClick={resetWizard}>
            取消
          </button>
        )}
      </div>

      <ol className="wizard-steps" aria-label="配对步骤">
        {steps.map((s, i) => (
          <li
            key={s.id}
            className={
              i < activeIdx
                ? 'done'
                : i === activeIdx
                  ? 'current'
                  : undefined
            }
          >
            <span className="wizard-num">{i + 1}</span>
            {s.label}
          </li>
        ))}
      </ol>

      {step === 'idle' ? (
        <p className="muted" style={{ marginTop: '0.75rem' }}>
          点击「添加设备」生成 QR / 配对码，用 App 扫描后在此批准。
        </p>
      ) : null}

      {step === 'scan' ? (
        <div className="wizard-body">
          <p className="muted">用手机 App 扫描下方二维码，或手动输入配对码。</p>
          {qrSvg ? (
            <div
              className="qr"
              aria-label="配对二维码"
              dangerouslySetInnerHTML={{ __html: qrSvg }}
            />
          ) : null}
          <dl className="pair-meta">
            {info.code ? (
              <>
                <dt>配对码</dt>
                <dd>
                  <code>{info.code}</code>
                </dd>
              </>
            ) : null}
            {info.local ? (
              <>
                <dt>本机</dt>
                <dd className="mono">{info.local}</dd>
              </>
            ) : null}
            {info.channel ? (
              <>
                <dt>信道</dt>
                <dd className="mono">{info.channel}</dd>
              </>
            ) : null}
            {info.fingerprint ? (
              <>
                <dt>指纹</dt>
                <dd className="mono">{shortId(info.fingerprint)}</dd>
              </>
            ) : null}
          </dl>
          <p className="muted">等待扫码中…页面会自动进入批准步骤。</p>
        </div>
      ) : null}

      {step === 'approve' && pending ? (
        <div className="pair-pending">
          <strong>有设备请求配对，请确认</strong>
          <div className="row">
            <span>
              {pending.device_name || ''} ·{' '}
              {shortId(pending.fingerprint || '')}
            </span>
            <button
              type="button"
              className="primary"
              disabled={busy}
              onClick={() => void decide(true)}
            >
              批准
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={() => void decide(false)}
            >
              拒绝
            </button>
          </div>
        </div>
      ) : null}

      {step === 'done' && doneMsg ? (
        <p className={doneMsg.includes('批准') || doneMsg.includes('成功') ? 'ok' : 'muted'}>
          {doneMsg}
        </p>
      ) : null}

      <h3 className="subhead">已配对设备</h3>
      {!peers.length && !(masterId && masterId === selfId) ? (
        <p className="muted">暂无已配对设备</p>
      ) : (
        <table className="data">
          <thead>
            <tr>
              <th>名称</th>
              <th>fingerprint</th>
              <th>角色</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {masterId && masterId === selfId ? (
              <tr>
                <td>本机</td>
                <td>{shortId(selfId)}</td>
                <td>
                  <StatusPill tone="ok">master</StatusPill>
                </td>
                <td />
              </tr>
            ) : null}
            {peers.map((p) => {
              const isMaster = !!p.fingerprint && p.fingerprint === masterId;
              return (
                <tr key={p.fingerprint || p.device_name}>
                  <td>{p.device_name || ''}</td>
                  <td>{shortId(p.fingerprint || '')}</td>
                  <td>
                    {isMaster ? (
                      <StatusPill tone="ok">master</StatusPill>
                    ) : (
                      <StatusPill>peer</StatusPill>
                    )}
                  </td>
                  <td>
                    <button
                      type="button"
                      onClick={() => void unpair(p.fingerprint || '')}
                    >
                      解除配对
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
