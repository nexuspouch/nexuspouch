import { useState, type ReactNode } from 'react';
import { api } from '../../shared/api';
import { shortId } from '../../shared/format';
import type { MdnsPeer } from '../types';

type Props = { onError: (e: unknown) => void };

type Diag = {
  local?: { ok?: boolean; latency_ms?: number; error?: string };
  lan?: {
    ok?: boolean;
    discovered?: number;
    error?: string;
    peers?: MdnsPeer[];
  };
  channel?: {
    configured?: boolean;
    ok?: boolean;
    host?: string;
    port?: number;
    dns_ok?: boolean;
    tcp_ok?: boolean;
    ws_ok?: boolean;
    ws_status?: string;
    latency_ms?: number;
    error?: string;
    note?: string;
  };
  mdns_advertising?: boolean;
};

function StatusLine({
  label,
  ok,
  detail,
}: {
  label: string;
  ok: boolean;
  detail?: string;
}) {
  return (
    <div>
      <strong>{label}</strong>{' '}
      <span className={ok ? 'ok' : 'err'}>{ok ? '✓' : '✗'}</span>
      {detail ? <span className="muted"> {detail}</span> : null}
    </div>
  );
}

function MdnsTable({ peers }: { peers: MdnsPeer[] }) {
  if (!peers.length) return <p className="muted">未发现局域网节点</p>;
  return (
    <table className="data">
      <thead>
        <tr>
          <th>名称</th>
          <th>fp</th>
          <th>endpoint</th>
        </tr>
      </thead>
      <tbody>
        {peers.map((p) => (
          <tr key={`${p.fingerprint}-${p.endpoint}`}>
            <td>
              {p.name || ''}
              {p.same_host ? <span className="muted"> (本机)</span> : null}
            </td>
            <td>{shortId(p.fingerprint || '')}</td>
            <td style={{ wordBreak: 'break-all', fontSize: '0.85rem' }}>
              {p.endpoint || ''}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export function DiscoveryPanel({ onError }: Props) {
  const [status, setStatus] = useState(
    '点击「诊断」检查本机 / 局域网 / Channel 连通性。',
  );
  const [details, setDetails] = useState<ReactNode>(null);
  const [mdns, setMdns] = useState<ReactNode>(
    <span className="muted">点击「刷新发现」浏览局域网节点。</span>,
  );

  async function runDiagnostics() {
    setStatus('诊断中…');
    try {
      const d = await api<Diag>('/admin/api/diagnostics');
      const local = d.local || {};
      const lan = d.lan || {};
      const ch = d.channel || {};
      const nodes: ReactNode[] = [
        <StatusLine
          key="local"
          label="本机"
          ok={!!local.ok}
          detail={`${local.latency_ms != null ? `${local.latency_ms} ms` : ''}${
            local.error ? ` · ${local.error}` : ''
          }`}
        />,
        <StatusLine
          key="lan"
          label="局域网"
          ok={!!lan.ok}
          detail={`发现 ${lan.discovered || 0} 个${lan.error ? ` · ${lan.error}` : ''}`}
        />,
      ];
      if (ch.configured) {
        const parts = [
          ch.host
            ? `${ch.host}${ch.port != null ? `:${ch.port}` : ''}`
            : '',
          `dns ${ch.dns_ok ? 'ok' : 'fail'}`,
          `tcp ${ch.tcp_ok ? 'ok' : 'fail'}`,
          `ws ${ch.ws_ok ? 'ok' : 'fail'}`,
          ch.ws_status,
          ch.latency_ms != null ? `${ch.latency_ms} ms` : '',
          ch.error,
        ].filter(Boolean);
        nodes.push(
          <StatusLine key="ch" label="Channel" ok={!!ch.ok} detail={parts.join(' · ')} />,
        );
        if (ch.note) {
          nodes.push(
            <div key="note" className="muted" style={{ marginLeft: '1rem' }}>
              {ch.note}
            </div>,
          );
        }
      } else {
        nodes.push(
          <div key="ch-skip">
            <strong>Channel</strong> <span className="muted">未配置（跳过）</span>
          </div>,
        );
      }
      nodes.push(
        <div key="mdns" className="muted" style={{ marginTop: '0.35rem' }}>
          mDNS 广播:{' '}
          {d.mdns_advertising ? (
            <span className="ok">是</span>
          ) : (
            <span className="err">否</span>
          )}
        </div>,
      );
      setDetails(nodes);
      setStatus('');
      if (lan.peers?.length) setMdns(<MdnsTable peers={lan.peers} />);
    } catch (e) {
      setStatus('');
      onError(e);
    }
  }

  async function refreshDiscovery() {
    setMdns(<span className="muted">浏览中…</span>);
    try {
      const out = await api<{ peers?: MdnsPeer[] }>('/admin/api/discovery');
      setMdns(<MdnsTable peers={out.peers || []} />);
    } catch (e) {
      setMdns(null);
      onError(e);
    }
  }

  return (
    <section className="panel">
      <div className="section-head">
        <h2>连通性 / 发现</h2>
        <div className="row">
          <button type="button" onClick={() => void runDiagnostics()}>
            诊断
          </button>
          <button type="button" onClick={() => void refreshDiscovery()}>
            刷新发现
          </button>
        </div>
      </div>
      {status ? <div className="muted" style={{ marginBottom: '0.5rem' }}>{status}</div> : null}
      <div>{details}</div>
      <h3 className="subhead">mDNS 发现</h3>
      <div>{mdns}</div>
    </section>
  );
}
