import { useCallback, useEffect, useRef, useState } from 'react';
import { api } from '../../shared/api';
import { shortId } from '../../shared/format';
import type { PairPending, PairStart, Peer } from '../types';

type Props = {
  peers: Peer[];
  selfId: string;
  masterId: string;
  onError: (e: unknown) => void;
  onRefresh: () => Promise<void>;
};

export function PairingPanel({
  peers,
  selfId,
  masterId,
  onError,
  onRefresh,
}: Props) {
  const [qrSvg, setQrSvg] = useState('');
  const [info, setInfo] = useState(
    '点击「开始配对」生成 QR / 配对码，用 App 扫描后在此批准。',
  );
  const [pending, setPending] = useState<PairPending | null>(null);
  const [statusHtml, setStatusHtml] = useState<string | null>(null);
  const awaiting = useRef(false);
  const timer = useRef<ReturnType<typeof setInterval> | null>(null);
  const prevSid = useRef('');

  const stopPoll = useCallback(() => {
    if (timer.current) {
      clearInterval(timer.current);
      timer.current = null;
    }
  }, []);

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
          setStatusHtml(null);
          setQrSvg('');
          setInfo('配对已结束。可再次点击「开始配对」。');
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
      setStatusHtml(null);
    } catch {
      /* ignore */
    }
  }, [onRefresh, stopPoll]);

  useEffect(() => {
    void updatePending();
    return () => stopPoll();
  }, [updatePending, stopPoll]);

  async function startPair() {
    try {
      const out = await api<PairStart>('/admin/api/pairing/start', {
        method: 'POST',
      });
      prevSid.current = '';
      setPending(null);
      setStatusHtml(null);
      setQrSvg(out.qr_svg || '');
      setInfo(
        `code=${out.code}\nlocal=${out.local_endpoint}${
          out.channel_endpoint ? `\nchannel=${out.channel_endpoint}` : ''
        }\nfp=${out.fingerprint}\n\n${out.qr}\n\n等待手机扫码…（页面会自动弹出批准）`,
      );
      awaiting.current = true;
      stopPoll();
      timer.current = setInterval(() => {
        void updatePending();
      }, 1000);
      await onRefresh();
    } catch (e) {
      onError(e);
    }
  }

  async function decide(accept: boolean) {
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
      setStatusHtml(accept ? 'ok:已批准' : 'muted:已拒绝');
      await onRefresh();
    } catch (e) {
      onError(e);
    }
  }

  async function unpair(fp: string) {
    if (!confirm(`解除配对 ${shortId(fp)}？`)) return;
    try {
      await api('/admin/api/peers/remove', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ fingerprint: fp }),
      });
      await onRefresh();
    } catch (e) {
      onError(e);
    }
  }

  return (
    <section className="panel">
      <div className="section-head">
        <h2>Noise 配对</h2>
        <button type="button" className="primary" onClick={() => void startPair()}>
          添加设备
        </button>
      </div>
      {qrSvg ? (
        <div className="qr" aria-label="配对二维码" dangerouslySetInnerHTML={{ __html: qrSvg }} />
      ) : (
        <div className="qr" />
      )}
      <pre className="block muted">{info}</pre>
      {statusHtml?.startsWith('ok:') ? (
        <p className="ok">{statusHtml.slice(3)}</p>
      ) : null}
      {statusHtml?.startsWith('muted:') ? (
        <p className="muted">{statusHtml.slice(6)}</p>
      ) : null}
      {pending ? (
        <div className="pair-pending">
          <strong>有设备请求配对，请确认</strong>
          <div className="row">
            <span>
              {pending.device_name || ''} · {shortId(pending.fingerprint || '')}
            </span>
            <button type="button" className="primary" onClick={() => void decide(true)}>
              批准
            </button>
            <button type="button" onClick={() => void decide(false)}>
              拒绝
            </button>
          </div>
        </div>
      ) : null}

      <h3 className="subhead">已配对设备</h3>
      {!peers.length ? (
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
                  <span className="ok">master</span>
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
                  <td>{isMaster ? <span className="ok">master</span> : 'peer'}</td>
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
