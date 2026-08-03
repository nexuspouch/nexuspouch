import { useCallback, useEffect, useState } from 'react';
import { api } from '../../shared/api';
import { useFeedback } from '../../shared/ui/feedback';
import type { BindingReport, BindingRow } from '../types';

const PRESETS = [
  {
    id: 'claude-code',
    label: 'Claude Code',
    external: '~/.claude/projects',
    folder: 'claude-code',
    hint: 'Claude Code 项目会话（~/.claude/projects）',
  },
  {
    id: 'codex',
    label: 'Codex',
    external: '~/.codex/sessions',
    folder: 'codex',
    hint: 'Codex 会话转写（~/.codex/sessions）',
  },
  {
    id: 'custom',
    label: '自定义',
    external: '',
    folder: '',
    hint: '任意本地目录 → sessions/<agent>',
  },
] as const;

export function BindPage({
  onNotice,
  onError,
}: {
  onNotice: (msg: string) => void;
  onError: (err: unknown) => void;
}) {
  const { toast, confirm } = useFeedback();
  const [preset, setPreset] = useState<string>('claude-code');
  const [external, setExternal] = useState('~/.claude/projects');
  const [folder, setFolder] = useState('claude-code');
  const [hint, setHint] = useState<string>(PRESETS[0].hint);
  const [bindings, setBindings] = useState<BindingRow[]>([]);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const out = await api<{ bindings?: BindingRow[] }>('/admin/api/bindings');
      setBindings((out.bindings || []).filter((b) => b.space === 'sessions'));
    } catch (e) {
      onError(e);
    }
  }, [onError]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const applyPreset = (id: string) => {
    setPreset(id);
    const p = PRESETS.find((x) => x.id === id) || PRESETS[0];
    if (p.id !== 'custom') {
      setExternal(p.external);
      setFolder(p.folder);
    }
    setHint(p.hint);
  };

  return (
    <>
      <div className="panel">
        <h2 style={{ marginTop: 0 }}>绑定 agent 会话目录</h2>
        <p className="hint">
          把本机 agent 会话目录摄进 sessions 空间。绑定后立即同步；之后约每 60
          秒自动扫描。路径支持 ~/。
        </p>
        <div className="form-row">
          <select value={preset} onChange={(e) => applyPreset(e.target.value)}>
            {PRESETS.map((p) => (
              <option key={p.id} value={p.id}>
                {p.label}
              </option>
            ))}
          </select>
          <input
            value={external}
            onChange={(e) => setExternal(e.target.value)}
            placeholder="外部目录"
            style={{ minWidth: '16rem', flex: 1 }}
          />
          <input
            value={folder}
            onChange={(e) => setFolder(e.target.value)}
            placeholder="agent 名"
            style={{ width: '9rem' }}
          />
          <button
            type="button"
            className="primary"
            disabled={busy}
            onClick={async () => {
              onError('');
              if (!external.trim() || !folder.trim()) {
                onError('请填写外部目录和 agent 名');
                return;
              }
              setBusy(true);
              try {
                const out = await api<{
                  binding?: { external?: string };
                  report?: BindingReport;
                }>('/admin/api/bindings', {
                  method: 'POST',
                  headers: { 'Content-Type': 'application/json' },
                  body: JSON.stringify({
                    external: external.trim(),
                    space: 'sessions',
                    folder: folder.trim(),
                    mode: 'auto',
                    label: `${folder.trim()}@admin`,
                    sync: true,
                  }),
                });
                const r = out.report || {};
                onNotice(
                  `已绑定 ${out.binding?.external || external} → sessions/${folder.trim()}` +
                    ` · 新增 ${r.added || 0} / 更新 ${r.updated || 0}`,
                );
                await refresh();
              } catch (e) {
                onError(e);
              } finally {
                setBusy(false);
              }
            }}
          >
            绑定并同步
          </button>
        </div>
        <p className="hint">{hint}</p>
      </div>

      <div className="panel" style={{ marginTop: 14 }}>
        <div className="toolbar">
          <h2 style={{ margin: 0 }}>已有绑定</h2>
          <button
            type="button"
            disabled={busy}
            onClick={async () => {
              setBusy(true);
              onError('');
              try {
                const out = await api<{ reports?: BindingReport[] }>(
                  '/admin/api/bindings/sync',
                  {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: '{}',
                  },
                );
                const reports = out.reports || [];
                const added = reports.reduce((n, r) => n + (r.added || 0), 0);
                const updated = reports.reduce((n, r) => n + (r.updated || 0), 0);
                onNotice(`同步完成：新增 ${added} / 更新 ${updated}`);
                await refresh();
              } catch (e) {
                onError(e);
              } finally {
                setBusy(false);
              }
            }}
          >
            全部同步
          </button>
        </div>
        {!bindings.length ? (
          <p className="hint">还没有 sessions 绑定。用上方预设添加 Claude / Codex。</p>
        ) : (
          <table className="data">
            <thead>
              <tr>
                <th>外部目录</th>
                <th>目标</th>
                <th>模式</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {bindings.map((b) => (
                <tr key={b.id}>
                  <td>
                    <code>{b.external}</code>
                  </td>
                  <td>
                    {b.space}/{b.folder}
                  </td>
                  <td>{b.mode || 'auto'}</td>
                  <td>
                    <button
                      type="button"
                      className="danger"
                      onClick={async () => {
                        const ok = await confirm({
                          title: '移除绑定',
                          message: `移除绑定 ${b.folder}？已入库的会话文件不会删除。`,
                          confirmLabel: '移除',
                          danger: true,
                        });
                        if (!ok) return;
                        try {
                          await api('/admin/api/bindings/remove', {
                            method: 'POST',
                            headers: { 'Content-Type': 'application/json' },
                            body: JSON.stringify({ id: b.id }),
                          });
                          onNotice(`已移除绑定 ${b.id}`);
                          toast(`已移除绑定 ${b.id}`, { kind: 'ok' });
                          await refresh();
                        } catch (e) {
                          onError(e);
                        }
                      }}
                    >
                      移除
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </>
  );
}
