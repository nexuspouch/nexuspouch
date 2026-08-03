import { useState } from 'react';
import type { RecycleEntry } from '../types';
import { BrowsePanel } from './BrowsePanel';
import { RecyclePanel } from './RecyclePanel';

type Tab = 'browse' | 'recycle';

type Props = {
  selfId: string;
  deviceIds: string[];
  peerNames: Record<string, string>;
  recycle: RecycleEntry[];
  onError: (e: unknown) => void;
  onRefresh: () => Promise<void>;
  initialTab?: Tab;
  tab?: Tab;
  onTabChange?: (tab: Tab) => void;
};

export function StoragePanel({
  selfId,
  deviceIds,
  peerNames,
  recycle,
  onError,
  onRefresh,
  initialTab = 'browse',
  tab: controlledTab,
  onTabChange,
}: Props) {
  const [internalTab, setInternalTab] = useState<Tab>(initialTab);
  const tab = controlledTab ?? internalTab;

  function setTab(next: Tab) {
    if (onTabChange) onTabChange(next);
    else setInternalTab(next);
  }

  return (
    <section className="panel" id="storage">
      <div className="section-head">
        <h2>存储</h2>
        <div className="tabs" role="tablist" aria-label="存储">
          <button
            type="button"
            role="tab"
            aria-selected={tab === 'browse'}
            className={tab === 'browse' ? 'active' : undefined}
            onClick={() => setTab('browse')}
          >
            浏览
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={tab === 'recycle'}
            className={tab === 'recycle' ? 'active' : undefined}
            onClick={() => setTab('recycle')}
          >
            回收站{recycle.length ? ` (${recycle.length})` : ''}
          </button>
        </div>
      </div>
      {tab === 'browse' ? (
        <BrowsePanel
          selfId={selfId}
          deviceIds={deviceIds}
          peerNames={peerNames}
          onError={onError}
          onRefresh={onRefresh}
          onGoRecycle={() => setTab('recycle')}
        />
      ) : (
        <RecyclePanel
          entries={recycle}
          onError={onError}
          onRefresh={onRefresh}
          onRestored={() => setTab('browse')}
        />
      )}
    </section>
  );
}
