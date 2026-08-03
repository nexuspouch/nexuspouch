import { useCallback, useEffect, useMemo, useState } from 'react';
import { api } from '../../shared/api';
import { fmtBytes, shortId } from '../../shared/format';
import { useFeedback } from '../../shared/ui/feedback';
import type { BrowseEntry } from '../types';

type Props = {
  selfId: string;
  deviceIds: string[];
  peerNames: Record<string, string>;
  onError: (e: unknown) => void;
  onRefresh: () => Promise<void>;
  onGoRecycle?: () => void;
};

const SPACES = ['files', 'artifacts', 'attachments', 'backups'] as const;

type DirChild = { name: string; size: number };
type FileChild = { path: string; name: string; size: number };

function synthesize(
  entries: BrowseEntry[],
  currentPath: string,
): { dirs: DirChild[]; files: FileChild[] } {
  const prefix = currentPath ? `${currentPath.replace(/\/$/, '')}/` : '';
  const dirMap = new Map<string, number>();
  const files: FileChild[] = [];

  for (const e of entries) {
    const full = e.path || '';
    let rest = full;
    if (currentPath) {
      if (full === currentPath) continue;
      if (!full.startsWith(prefix)) continue;
      rest = full.slice(prefix.length);
    }
    if (!rest) continue;
    const slash = rest.indexOf('/');
    const size = Number(e.size) || 0;
    if (slash === -1) {
      files.push({ path: full, name: rest, size });
    } else {
      const name = rest.slice(0, slash);
      dirMap.set(name, (dirMap.get(name) || 0) + size);
    }
  }

  const dirs = [...dirMap.entries()]
    .map(([name, size]) => ({ name, size }))
    .sort((a, b) => a.name.localeCompare(b.name));
  files.sort((a, b) => a.name.localeCompare(b.name));
  return { dirs, files };
}

export function BrowsePanel({
  selfId,
  deviceIds,
  peerNames,
  onError,
  onRefresh,
  onGoRecycle,
}: Props) {
  const { toast, confirm } = useFeedback();

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
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(() => new Set());

  useEffect(() => {
    if (!device && browseIds.length) setDevice(selfId || browseIds[0]);
    else if (device && browseIds.length && !browseIds.includes(device)) {
      setDevice(selfId || browseIds[0]);
    }
  }, [browseIds, device, selfId]);

  const loadBrowse = useCallback(async () => {
    if (!device) return;
    setLoading(true);
    try {
      const q = new URLSearchParams({ device, space });
      const p = path.trim();
      if (p) q.set('path', p);
      const out = await api<{ entries?: BrowseEntry[] }>(
        `/admin/api/browse?${q.toString()}`,
      );
      setEntries(out.entries || []);
      setSelected(new Set());
    } catch (e) {
      onError(e);
    } finally {
      setLoading(false);
    }
  }, [device, space, path, onError]);

  useEffect(() => {
    void loadBrowse();
  }, [loadBrowse]);

  const { dirs, files } = useMemo(
    () => synthesize(entries || [], path.trim()),
    [entries, path],
  );

  const crumbs = useMemo(() => {
    const parts = path.trim() ? path.trim().split('/').filter(Boolean) : [];
    const items: { label: string; path: string }[] = [
      { label: space, path: '' },
    ];
    let acc = '';
    for (const part of parts) {
      acc = acc ? `${acc}/${part}` : part;
      items.push({ label: part, path: acc });
    }
    return items;
  }, [path, space]);

  const allFilePaths = useMemo(() => files.map((f) => f.path), [files]);
  const allSelected =
    allFilePaths.length > 0 && allFilePaths.every((p) => selected.has(p));

  function toggleAll() {
    if (allSelected) setSelected(new Set());
    else setSelected(new Set(allFilePaths));
  }

  function toggleOne(p: string) {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(p)) next.delete(p);
      else next.add(p);
      return next;
    });
  }

  function enterDir(name: string) {
    setPath((prev) => (prev ? `${prev.replace(/\/$/, '')}/${name}` : name));
  }

  async function deletePaths(paths: string[]) {
    if (!paths.length) return;
    const label =
      paths.length === 1
        ? `删除 ${paths[0]}？将移入回收站。`
        : `删除选中的 ${paths.length} 项？将移入回收站。`;
    const ok = await confirm({
      title: '移入回收站',
      message: label,
      confirmLabel: '删除',
      danger: true,
    });
    if (!ok) return;

    setBusy(true);
    let failed = 0;
    try {
      for (const p of paths) {
        try {
          await api('/admin/api/browse/delete', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ device, space, path: p }),
          });
        } catch {
          failed += 1;
        }
      }
      await loadBrowse();
      await onRefresh();
      if (failed) {
        toast(`删除完成，${failed} 项失败`, { kind: 'err' });
      } else {
        toast(
          paths.length === 1 ? '已移入回收站' : `已移入回收站（${paths.length}）`,
          {
            kind: 'ok',
            action: onGoRecycle
              ? { label: '查看回收站', onClick: onGoRecycle }
              : undefined,
          },
        );
      }
    } catch (e) {
      onError(e);
    } finally {
      setBusy(false);
    }
  }

  function labelFor(id: string): string {
    if (id === selfId) return `${shortId(id)} (本机)`;
    if (peerNames[id]) return `${peerNames[id]} · ${shortId(id)}`;
    return shortId(id);
  }

  return (
    <div className="browse-pane">
      <p className="muted" style={{ marginTop: 0 }}>
        浏览本机上的设备镜像（含已配对设备）。点目录进入，删除进回收站。
      </p>
      <div className="row">
        <label>
          设备{' '}
          <select
            value={device}
            onChange={(e) => {
              setPath('');
              setDevice(e.target.value);
            }}
          >
            {browseIds.map((id) => (
              <option key={id} value={id}>
                {labelFor(id)}
              </option>
            ))}
          </select>
        </label>
        <label>
          分区{' '}
          <select
            value={space}
            onChange={(e) => {
              setPath('');
              setSpace(e.target.value);
            }}
          >
            {SPACES.map((s) => (
              <option key={s} value={s}>
                {s}
              </option>
            ))}
          </select>
        </label>
        <button
          type="button"
          disabled={loading || busy}
          onClick={() => void loadBrowse()}
        >
          {loading ? '加载中…' : '刷新'}
        </button>
        {selected.size > 0 ? (
          <button
            type="button"
            className="danger"
            disabled={busy}
            onClick={() => void deletePaths([...selected])}
          >
            删除选中（{selected.size}）
          </button>
        ) : null}
      </div>

      <nav className="breadcrumbs" aria-label="路径">
        {crumbs.map((c, i) => {
          const isLast = i === crumbs.length - 1;
          return (
            <span key={`${c.path}-${i}`}>
              {i > 0 ? <span className="sep">/</span> : null}
              <button
                type="button"
                className={isLast ? 'current' : undefined}
                disabled={isLast}
                onClick={() => setPath(c.path)}
              >
                {c.label}
              </button>
            </span>
          );
        })}
      </nav>

      {entries === null || (loading && entries === null) ? (
        <p className="muted">加载中…</p>
      ) : !dirs.length && !files.length ? (
        <p className="muted">此目录为空</p>
      ) : (
        <table className="data">
          <thead>
            <tr>
              <th style={{ width: '2rem' }}>
                <input
                  type="checkbox"
                  checked={allSelected}
                  disabled={!files.length || busy}
                  onChange={toggleAll}
                  aria-label="全选文件"
                />
              </th>
              <th>名称</th>
              <th>大小</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {dirs.map((d) => (
              <tr key={`d:${d.name}`}>
                <td />
                <td>
                  <button
                    type="button"
                    className="linkish"
                    onClick={() => enterDir(d.name)}
                  >
                    {d.name}/
                  </button>
                </td>
                <td>{fmtBytes(d.size)}</td>
                <td />
              </tr>
            ))}
            {files.map((f) => (
              <tr
                key={`f:${f.path}`}
                className={selected.has(f.path) ? 'selected' : undefined}
              >
                <td>
                  <input
                    type="checkbox"
                    checked={selected.has(f.path)}
                    disabled={busy}
                    onChange={() => toggleOne(f.path)}
                    aria-label={`选择 ${f.name}`}
                  />
                </td>
                <td className="mono">{f.name}</td>
                <td>{fmtBytes(f.size)}</td>
                <td>
                  <button
                    type="button"
                    className="danger"
                    disabled={busy}
                    onClick={() => void deletePaths([f.path])}
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
  );
}
