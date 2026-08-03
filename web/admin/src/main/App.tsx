import { useCallback, useEffect, useState } from 'react';
import { api, getToken, seedTokenFromQuery, setToken } from '../shared/api';
import { fmtBytes, shortId } from '../shared/format';
import { AgentsPanel } from './panels/AgentsPanel';
import { AuditPanel } from './panels/AuditPanel';
import { BrowsePanel } from './panels/BrowsePanel';
import { DangerPanel } from './panels/DangerPanel';
import { DiscoveryPanel } from './panels/DiscoveryPanel';
import { ImportPanel } from './panels/ImportPanel';
import { PairingPanel } from './panels/PairingPanel';
import { RecyclePanel } from './panels/RecyclePanel';
import { SpacesPanel } from './panels/SpacesPanel';
import { StatsPanel } from './panels/StatsPanel';
import { VersionsPanel } from './panels/VersionsPanel';
import type {
  ImportRequest,
  Peer,
  RecycleEntry,
  Stats,
} from './types';

export function App() {
  const [tokenDraft, setTokenDraft] = useState('');
  const [msg, setMsg] = useState('');
  const [stats, setStats] = useState<Stats | null>(null);
  const [peers, setPeers] = useState<Peer[]>([]);
  const [imports, setImports] = useState<ImportRequest[]>([]);
  const [issued, setIssued] = useState('…');
  const [received, setReceived] = useState('…');
  const [recycle, setRecycle] = useState<RecycleEntry[]>([]);
  const [peerNames, setPeerNames] = useState<Record<string, string>>({});
  const [tick, setTick] = useState(0);

  useEffect(() => {
    seedTokenFromQuery();
    setTokenDraft(getToken());
  }, []);

  const onError = useCallback((err: unknown) => {
    setMsg(String((err as Error)?.message || err || ''));
  }, []);

  const refresh = useCallback(async () => {
    setMsg('');
    try {
      const s = await api<Stats>('/admin/api/stats');
      setStats(s);

      const names: Record<string, string> = {};
      let plist: Peer[] = [];
      try {
        const peersOut = await api<{ peers?: Peer[] }>('/admin/api/peers');
        plist = peersOut.peers || [];
        for (const p of plist) {
          if (p.fingerprint) names[p.fingerprint] = p.device_name || '';
        }
      } catch {
        /* peers optional */
      }
      setPeers(plist);
      setPeerNames(names);

      const pending = await api<{ requests?: ImportRequest[] }>(
        '/admin/api/import/pending',
      );
      setImports(pending.requests || []);

      const issuedOut = await api<{ grants?: unknown[] }>(
        '/admin/api/import/grants?role=issued',
      );
      const issuedList = issuedOut.grants || [];
      setIssued(
        issuedList.length ? JSON.stringify(issuedList, null, 2) : '（无）',
      );

      const receivedOut = await api<{ grants?: unknown[] }>(
        '/admin/api/import/grants?role=received',
      );
      const receivedList = receivedOut.grants || [];
      setReceived(
        receivedList.length
          ? JSON.stringify(receivedList, null, 2)
          : '（无）',
      );

      const rec = await api<{ entries?: RecycleEntry[] }>('/admin/api/recycle');
      setRecycle(rec.entries || []);

      setTick((t) => t + 1);
    } catch (e) {
      onError(e);
    }
  }, [onError]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const volumeWarn =
    stats?.volume_warn &&
    `卷用量告警：已用约 ${Math.round((Number(stats.volume_used_ratio) || 0) * 100)}%（剩余 ${fmtBytes(Number(stats.volume_free_bytes))} / 共 ${fmtBytes(Number(stats.volume_total_bytes))}）。请清理镜像或扩大磁盘。`;

  const selfId = stats?.self_device || stats?.device || '';
  const masterId = stats?.master || '';
  const masterEpoch = stats?.master_epoch ?? 0;
  const devices = stats?.devices || {};

  return (
    <div className="app-shell admin-main">
      <header className="topbar">
        <div>
          <h1>ShePaw Storage Admin</h1>
          <p className="sub">
            无头节点管理面 · Noise 配对 / 浏览手删 / 用量 / 回收站 / 换机导入 ·{' '}
            <a href="/admin/sessions/">会话管理</a> ·{' '}
            <a href="/api/v1/events/recent">最近 store 事件</a>
          </p>
        </div>
      </header>

      {volumeWarn ? (
        <div className="banner volume show" role="alert">
          {volumeWarn}
        </div>
      ) : null}

      <div className="token-bar">
        <label>
          Token
          <input
            type="password"
            placeholder="admin token"
            value={tokenDraft}
            onChange={(e) => setTokenDraft(e.target.value)}
          />
        </label>
        <button
          type="button"
          className="primary"
          onClick={() => {
            setToken(tokenDraft);
            void refresh();
          }}
        >
          保存
        </button>
        <button type="button" onClick={() => void refresh()}>
          刷新
        </button>
      </div>

      {msg ? <p className="err">{msg}</p> : null}

      <PairingPanel
        peers={peers}
        selfId={selfId}
        masterId={masterId}
        onError={onError}
        onRefresh={refresh}
      />

      <DiscoveryPanel onError={onError} />

      <StatsPanel
        statsJson={stats ? JSON.stringify(stats, null, 2) : '…'}
        masterLabel={`master: ${shortId(masterId)} · epoch ${masterEpoch}${
          masterId === selfId ? ' (本机)' : ''
        }`}
        devices={devices}
        selfId={selfId}
        onError={onError}
        onRefresh={refresh}
      />

      <BrowsePanel
        selfId={selfId}
        deviceIds={Object.keys(devices)}
        peerNames={peerNames}
        onError={onError}
        onRefresh={refresh}
      />

      <ImportPanel
        requests={imports}
        issued={issued}
        received={received}
        onError={onError}
        onRefresh={refresh}
        onNotice={setMsg}
      />

      <RecyclePanel
        entries={recycle}
        onError={onError}
        onRefresh={refresh}
      />

      <AuditPanel refreshKey={tick} onError={onError} onRefresh={refresh} />

      <AgentsPanel refreshKey={tick} onError={onError} />

      <VersionsPanel refreshKey={tick} onError={onError} />

      <SpacesPanel refreshKey={tick} onError={onError} />

      <DangerPanel onError={onError} onRefresh={refresh} />
    </div>
  );
}
