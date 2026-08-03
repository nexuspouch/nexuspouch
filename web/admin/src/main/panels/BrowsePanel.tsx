import { useEffect, useMemo, useState } from 'react';
import { api } from '../../shared/api';
import { fmtBytes, shortId } from '../../shared/format';
import type { BrowseEntry } from '../types';

type Props = {
  selfId: string;
  deviceIds: string[];
  peerNames: Record<string, string>;
  onError: (e: unknown) => void;
  onRefresh: () => Promise<void>;
};

const SPACES = ['files', 'artifacts', 'attachments', 'backups'] as const;

export function BrowsePanel({
  selfId,
  deviceIds,
  peerNames,
  onError,
  onRefresh,
}: Props) {
  const browseIds = useMemo(() => {
    const set = new Set<string>();
    if (selfId) set.add(selfId);
    for (const id of deviceIds) set.add(id);
    for (const fp of Object.keys(peerNames)) set.add(fp);
    return [...set].sort((a, b) => {
      if (a === selfId) return -1;
      if (b === selfId) return 1;
      return a.localeCompare(b);
    });
  }, [selfId, deviceIds, peerNames]);

  const [device, setDevice] = useState('');
  const [space, setSpace] = useState<string>('files');
  const [path, setPath] = useState('');
  const [entries, setEntries] = useState<BrowseEntry[] | null>(null);

  useEffect(() => {
    if (!device && browseIds.length) setDevice(selfId || browseIds[0]);
    else if (device && browseIds.length && !browseIds.includes(device)) {
      setDevice(selfId || browseIds[0]);
    }
  }, [browseIds, device, selfId]);

  async function loadBrowse() {
    try {
      const q = new URLSearchParams({ device, space });
      const p = path.trim();
      if (p) q.set('path', p);
      const out = await api<{ entries?: BrowseEntry[] }>(
        `/admin/api/browse?${q.toString()}`,
      );
      setEntries(out.entries || []);
    } catch (e) {
      onError(e);
    }
  }

  async function del(p: string) {
    if (!confirm(`删除 ${p}？将移入回收站。`)) return;
    try {
      await api('/admin/api/browse/delete', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ device, space, path: p }),
      });
      await loadBrowse();
      await onRefresh();
    } catch (e) {
      onError(e);
    }
  }

  function labelFor(id: string): string {
    if (id === selfId) return `${shortId(id)} (本机)`;
    if (peerNames[id]) return `${peerNames[id]} · ${shortId(id)}`;
    return shortId(id);
  }

  return (
    <section className="panel">
      <h2>分区浏览</h2>
      <p className="muted">
        浏览本机上的设备镜像（含已配对设备）；未同步前列表可能为空。删除进回收站。
      </p>
      <div className="row">
        <label>
          设备{' '}
          <select value={device} onChange={(e) => setDevice(e.target.value)}>
            {browseIds.map((id) => (
              <option key={id} value={id}>
                {labelFor(id)}
              </option>
            ))}
          </select>
        </label>
        <label>
          分区{' '}
          <select value={space} onChange={(e) => setSpace(e.target.value)}>
            {SPACES.map((s) => (
              <option key={s} value={s}>
                {s}
              </option>
            ))}
          </select>
        </label>
        <label>
          前缀{' '}
          <input
            placeholder="可选子路径"
            style={{ minWidth: '10rem' }}
            value={path}
            onChange={(e) => setPath(e.target.value)}
          />
        </label>
        <button type="button" onClick={() => void loadBrowse()}>
          列出
        </button>
      </div>
      <div style={{ marginTop: '0.75rem' }}>
        {entries === null ? null : !entries.length ? (
          <p className="muted">无文件</p>
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
              {entries.map((e) => (
                <tr key={e.path}>
                  <td>{e.path || ''}</td>
                  <td>{fmtBytes(Number(e.size))}</td>
                  <td>
                    <button
                      type="button"
                      className="danger"
                      onClick={() => void del(e.path || '')}
                    >
                      删除
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </section>
  );
}
