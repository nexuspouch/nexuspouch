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

  const dirs = useMemo(
    () => (entries || []).filter((e) => e.is_dir),
    [entries],
  );
  const files = useMemo(
    () => (entries || []).filter((e) => !e.is_dir),
    [entries],
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

  const selectable = useMemo(
    () => (entries || []).map((e) => e.path || '').filter(Boolean),
    [entries],
  );
  const allSelected =
    selectable.length > 0 && selectable.every((p) => selected.has(p));

  function toggleAll() {
    if (allSelected) setSelected(new Set());
    else setSelected(new Set(selectable));
  }

  function toggleOne(p: string) {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(p)) next.delete(p);
      else next.add(p);
      return next;
    });
  }

  function enterDir(entry: BrowseEntry) {
    setPath(entry.path || '');
  }

  function nameOf(e: BrowseEntry): string {
    if (e.name) return e.name;
    const p = e.path || '';
    const i = p.lastIndexOf('/');
    return i >= 0 ? p.slice(i + 1) : p;
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
          paths.length === 1
            ? '已移入回收站'
            : `已移入回收站（${paths.length}）`,
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
        浅层浏览设备镜像。点目录进入；文件与目录均可删除进回收站。
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

      {entries === null ? (
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
                  disabled={!selectable.length || busy}
                  onChange={toggleAll}
                  aria-label="全选"
                />
              </th>
              <th>名称</th>
              <th>大小</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {dirs.map((d) => (
              <tr
                key={`d:${d.path}`}
                className={selected.has(d.path || '') ? 'selected' : undefined}
              >
                <td>
                  <input
                    type="checkbox"
                    checked={selected.has(d.path || '')}
                    disabled={busy}
                    onChange={() => toggleOne(d.path || '')}
                    aria-label={`选择目录 ${nameOf(d)}`}
                  />
                </td>
                <td>
                  <button
                    type="button"
                    className="linkish"
                    onClick={() => enterDir(d)}
                  >
                    {nameOf(d)}/
                  </button>
                </td>
                <td>{fmtBytes(Number(d.size))}</td>
                <td>
                  <button
                    type="button"
                    className="danger"
                    disabled={busy}
                    onClick={() => void deletePaths([d.path || ''])}
                  >
                    删除
                  </button>
                </td>
              </tr>
            ))}
            {files.map((f) => (
              <tr
                key={`f:${f.path}`}
                className={selected.has(f.path || '') ? 'selected' : undefined}
              >
                <td>
                  <input
                    type="checkbox"
                    checked={selected.has(f.path || '')}
                    disabled={busy}
                    onChange={() => toggleOne(f.path || '')}
                    aria-label={`选择 ${nameOf(f)}`}
                  />
                </td>
                <td className="mono">{nameOf(f)}</td>
                <td>{fmtBytes(Number(f.size))}</td>
                <td>
                  <button
                    type="button"
                    className="danger"
                    disabled={busy}
                    onClick={() => void deletePaths([f.path || ''])}
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
