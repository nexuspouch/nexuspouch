import { useCallback, useEffect, useState } from 'react';
import { api, getToken, seedTokenFromQuery, setToken } from '../shared/api';
import { fmtBytes, shortId } from '../shared/format';
import { useFeedback } from '../shared/ui/feedback';
import { AgentsPanel } from './panels/AgentsPanel';
import { AuditPanel } from './panels/AuditPanel';
import { DangerPanel } from './panels/DangerPanel';
import { DiscoveryPanel } from './panels/DiscoveryPanel';
import { ImportPanel } from './panels/ImportPanel';
import { PairingPanel } from './panels/PairingPanel';
import { SpacesPanel } from './panels/SpacesPanel';
import { StatsPanel } from './panels/StatsPanel';
import { StoragePanel } from './panels/StoragePanel';
import { VersionsPanel } from './panels/VersionsPanel';
import {
  parseSection,
  SECTIONS,
  sectionHash,
  type SectionId,
} from './sections';
import type {
  ImportGrant,
  ImportRequest,
  Peer,
  RecycleEntry,
  Stats,
} from './types';

export function App() {
  const { toast } = useFeedback();
  const [tokenDraft, setTokenDraft] = useState('');
  const [msg, setMsg] = useState('');
  const [stats, setStats] = useState<Stats | null>(null);
  const [peers, setPeers] = useState<Peer[]>([]);
  const [imports, setImports] = useState<ImportRequest[]>([]);
  const [issued, setIssued] = useState<ImportGrant[]>([]);
  const [received, setReceived] = useState<ImportGrant[]>([]);
  const [recycle, setRecycle] = useState<RecycleEntry[]>([]);
  const [peerNames, setPeerNames] = useState<Record<string, string>>({});
  const [tick, setTick] = useState(0);
  const [storageTab, setStorageTab] = useState<'browse' | 'recycle'>('browse');
  const [section, setSection] = useState<SectionId>(() => parseSection());

  useEffect(() => {
    seedTokenFromQuery();
    setTokenDraft(getToken());
  }, []);

  useEffect(() => {
    const onHash = () => setSection(parseSection());
    window.addEventListener('hashchange', onHash);
    if (!location.hash || location.hash === '#') {
      history.replaceState(null, '', sectionHash('overview'));
    }
    return () => window.removeEventListener('hashchange', onHash);
  }, []);

  const goSection = useCallback((id: SectionId) => {
    if (location.hash !== sectionHash(id)) {
      location.hash = sectionHash(id);
    } else {
      setSection(id);
    }
  }, []);

  const onError = useCallback(
    (err: unknown) => {
      const text = String((err as Error)?.message || err || '');
      setMsg(text);
      if (text) toast(text, { kind: 'err' });
    },
    [toast],
  );

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

      const issuedOut = await api<{ grants?: ImportGrant[] }>(
        '/admin/api/import/grants?role=issued',
      );
      setIssued(issuedOut.grants || []);

      const receivedOut = await api<{ grants?: ImportGrant[] }>(
        '/admin/api/import/grants?role=received',
      );
      setReceived(receivedOut.grants || []);

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

  const goStorage = useCallback(
    (tab: 'browse' | 'recycle' = 'browse') => {
      setStorageTab(tab);
      goSection('storage');
    },
    [goSection],
  );

  const volumeWarn =
    stats?.volume_warn &&
    `卷用量告警：已用约 ${Math.round((Number(stats.volume_used_ratio) || 0) * 100)}%（剩余 ${fmtBytes(Number(stats.volume_free_bytes))} / 共 ${fmtBytes(Number(stats.volume_total_bytes))}）。请清理镜像或扩大磁盘。`;

  const selfId = stats?.self_device || stats?.device || '';
  const masterId = stats?.master || '';
  const masterEpoch = stats?.master_epoch ?? 0;
  const devices = stats?.devices || {};

  const badges: Partial<Record<SectionId, number>> = {
    storage: recycle.length,
    import: imports.length,
  };

  return (
    <div className="app-shell admin-main">
      <header className="topbar">
        <div>
          <h1>ShePaw Storage Admin</h1>
          <p className="sub">
            无头节点管理面 ·{' '}
            <a href="/admin/sessions/">会话管理</a> ·{' '}
            <a href="/api/v1/events/recent">最近 store 事件</a>
          </p>
        </div>
      </header>

      {volumeWarn ? (
        <div className="banner volume show" role="alert">
          {volumeWarn}
          <span className="banner-actions">
            <button type="button" onClick={() => goStorage('recycle')}>
              去回收站
            </button>
            <button type="button" onClick={() => goStorage('browse')}>
              去浏览
            </button>
          </span>
        </div>
      ) : null}

      <div className="token-bar sticky-bar">
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
            toast('Token 已保存', { kind: 'ok' });
          }}
        >
          保存
        </button>
        <button type="button" onClick={() => void refresh()}>
          刷新
        </button>
      </div>

      <nav className="nav section-nav" aria-label="管理分区">
        {SECTIONS.map((s) => {
          const count = badges[s.id];
          return (
            <a
              key={s.id}
              href={sectionHash(s.id)}
              className={section === s.id ? 'active' : undefined}
              onClick={(e) => {
                e.preventDefault();
                goSection(s.id);
              }}
            >
              {s.label}
              {count ? <span className="nav-count">{count}</span> : null}
            </a>
          );
        })}
      </nav>

      {msg ? <p className="err">{msg}</p> : null}

      <div className="section-body">
        {section === 'overview' ? (
          <StatsPanel
            statsJson={stats ? JSON.stringify(stats, null, 2) : '…'}
            masterLabel={`master: ${shortId(masterId)} · epoch ${masterEpoch}${
              masterId === selfId ? ' (本机)' : ''
            }`}
            devices={devices}
            selfId={selfId}
            onError={onError}
            onRefresh={refresh}
            onGoStorage={() => goStorage('recycle')}
          />
        ) : null}

        {section === 'devices' ? (
          <>
            <PairingPanel
              peers={peers}
              selfId={selfId}
              masterId={masterId}
              onError={onError}
              onRefresh={refresh}
            />
            <DiscoveryPanel onError={onError} />
          </>
        ) : null}

        {section === 'storage' ? (
          <StoragePanel
            selfId={selfId}
            deviceIds={Object.keys(devices)}
            peerNames={peerNames}
            recycle={recycle}
            onError={onError}
            onRefresh={refresh}
            tab={storageTab}
            onTabChange={setStorageTab}
          />
        ) : null}

        {section === 'import' ? (
          <ImportPanel
            requests={imports}
            issued={issued}
            received={received}
            onError={onError}
            onRefresh={refresh}
            onNotice={(m) => {
              setMsg(m);
            }}
          />
        ) : null}

        {section === 'access' ? (
          <>
            <AuditPanel
              refreshKey={tick}
              onError={onError}
              onRefresh={refresh}
            />
            <AgentsPanel refreshKey={tick} onError={onError} />
          </>
        ) : null}

        {section === 'catalog' ? (
          <>
            <SpacesPanel refreshKey={tick} onError={onError} />
            <VersionsPanel refreshKey={tick} onError={onError} />
          </>
        ) : null}

        {section === 'danger' ? (
          <DangerPanel onError={onError} onRefresh={refresh} />
        ) : null}
      </div>
    </div>
  );
}
