import { api } from '../../shared/api';
import { fmtBytes, shortId } from '../../shared/format';
import { useFeedback } from '../../shared/ui/feedback';

type Props = {
  statsJson: string;
  masterLabel: string;
  devices: Record<string, Record<string, number>>;
  selfId: string;
  onError: (e: unknown) => void;
  onRefresh: () => Promise<void>;
  onGoStorage?: () => void;
};

export function StatsPanel({
  statsJson,
  masterLabel,
  devices,
  selfId,
  onError,
  onRefresh,
  onGoStorage,
}: Props) {
  const { toast, confirm } = useFeedback();
  const ids = Object.keys(devices);

  async function promoteMaster() {
    const ok = await confirm({
      title: '升为本机 master',
      message:
        '将本节点升为 master？在线会话会收到 master.pointer；离线端上线后经 pointer.query 改指。旧 master 不可达时可能有镜像缺口。',
      confirmLabel: '升主',
      danger: true,
    });
    if (!ok) return;
    try {
      const out = await api<{
        master?: string;
        epoch?: number;
        broadcast_peers?: number;
        seeded_files?: number;
        old_master_reachable?: boolean;
        dial_error?: string;
        hash_gate?: { ran?: boolean; mismatch_count?: number; ok?: boolean };
      }>('/admin/api/master/migrate', { method: 'POST' });
      const detail = [
        `${shortId(out.master || '')} · epoch ${out.epoch}`,
        `推送 ${out.broadcast_peers || 0}`,
        `种子 ${out.seeded_files || 0}`,
        out.old_master_reachable ? '旧 master 在线' : '旧 master 未在线',
        out.dial_error ? `拨号失败: ${out.dial_error}` : '',
        out.hash_gate?.ran
          ? `哈希门闩 mismatches=${out.hash_gate.mismatch_count || 0}${
              out.hash_gate.ok ? ' ok' : ' 有缺口'
            }`
          : '',
      ]
        .filter(Boolean)
        .join(' · ');
      toast(`已升主：${detail}`, { kind: 'ok', durationMs: 7000 });
      await onRefresh();
    } catch (e) {
      onError(e);
    }
  }

  async function runGc() {
    try {
      const out = await api<{ staging_removed?: number; recycle_bytes?: number }>(
        '/admin/api/gc',
        { method: 'POST' },
      );
      toast(
        `GC 完成：staging ${out.staging_removed || 0} · 回收站释放 ${fmtBytes(
          Number(out.recycle_bytes),
        )}`,
        {
          kind: 'ok',
          action: onGoStorage
            ? { label: '去存储', onClick: onGoStorage }
            : undefined,
        },
      );
      await onRefresh();
    } catch (e) {
      onError(e);
    }
  }

  async function purgeDev(id: string) {
    const ok = await confirm({
      title: '删除设备镜像',
      message: `永久删除设备 ${shortId(id)} 的镜像？不可还原。`,
      confirmLabel: '永久删除',
      danger: true,
    });
    if (!ok) return;
    try {
      await api('/admin/api/devices/purge', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ device_id: id }),
      });
      toast(`已删除镜像 ${shortId(id)}`, { kind: 'ok' });
      await onRefresh();
    } catch (e) {
      onError(e);
    }
  }

  return (
    <section className="panel">
      <div className="section-head">
        <h2>用量与 master</h2>
        <div className="row">
          <button type="button" onClick={() => void runGc()}>
            GC
          </button>
          <button type="button" onClick={() => void promoteMaster()}>
            升为本机 master
          </button>
        </div>
      </div>
      <p className="muted" style={{ marginTop: 0 }}>
        {masterLabel}
      </p>
      <p className="muted">
        启动时会自动 GC。升主时若旧 master 未入站会按配对端点拨号再 seed，并 fanout
        master.pointer；离线端靠重连 query 改指。
      </p>

      <h3 className="subhead">设备镜像</h3>
      <p className="muted">永久删除他端镜像目录（不可进回收站；禁删本机）。</p>
      {!ids.length ? (
        <p className="muted">无设备目录</p>
      ) : (
        <table className="data">
          <thead>
            <tr>
              <th>device</th>
              <th>占用</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {ids.map((id) => {
              const spaces = devices[id] || {};
              let total = 0;
              for (const k of Object.keys(spaces)) total += Number(spaces[k]) || 0;
              const isSelf = id === selfId;
              return (
                <tr key={id}>
                  <td>
                    {shortId(id)}
                    {isSelf ? <span className="muted"> (本机)</span> : null}
                  </td>
                  <td>{fmtBytes(total)}</td>
                  <td>
                    {!isSelf ? (
                      <button
                        type="button"
                        className="danger"
                        onClick={() => void purgeDev(id)}
                      >
                        删除镜像
                      </button>
                    ) : null}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}

      <details className="raw-details">
        <summary>原始 stats JSON</summary>
        <pre className="block">{statsJson}</pre>
      </details>
    </section>
  );
}
