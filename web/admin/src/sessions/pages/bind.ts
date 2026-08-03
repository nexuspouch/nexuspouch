import { api } from '../../shared/api';
import { clear, el, notice, setMsg, $ } from '../../shared/dom';
import type { BindingReport, BindingRow } from '../types';

const SESSION_PRESETS = [
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

async function renderBindingsList(box: HTMLElement): Promise<void> {
  clear(box);
  try {
    const out = await api<{ bindings?: BindingRow[] }>('/admin/api/bindings');
    const items = (out.bindings || []).filter((b) => b.space === 'sessions');
    if (!items.length) {
      box.appendChild(
        el('p', 'muted', '还没有 sessions 绑定。用上方预设添加 Claude / Codex 目录。'),
      );
      return;
    }
    const table = el('table');
    table.innerHTML =
      '<thead><tr><th>外部目录</th><th>目标</th><th>模式</th><th></th></tr></thead>';
    const tbody = el('tbody');
    for (const b of items) {
      const tr = el('tr');
      tr.appendChild(el('td', undefined, b.external));
      tr.appendChild(el('td', undefined, `${b.space}/${b.folder}`));
      tr.appendChild(el('td', undefined, b.mode || 'auto'));
      const td = el('td');
      const rm = el('button', undefined, '移除');
      rm.type = 'button';
      rm.onclick = async () => {
        if (!confirm(`移除绑定 ${b.folder}？已入库的会话文件不会删除。`)) return;
        try {
          await api('/admin/api/bindings/remove', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ id: b.id }),
          });
          notice(`已移除绑定 ${b.id}`);
          await renderBindingsList(box);
        } catch (e) {
          setMsg(e);
        }
      };
      td.appendChild(rm);
      tr.appendChild(td);
      tbody.appendChild(tr);
    }
    table.appendChild(tbody);
    box.appendChild(table);
  } catch (e) {
    setMsg(e);
  }
}

export async function showBind(): Promise<void> {
  const app = $('app');
  clear(app);

  const card = el('div', 'card');
  card.appendChild(el('h2', undefined, '绑定 agent 会话目录'));
  card.appendChild(
    el(
      'p',
      'muted',
      '把本机 agent 的会话目录摄进 sessions 空间。绑定后立即同步一次；之后约每 60 秒自动扫描。路径支持 ~/。',
    ),
  );

  const form = el('div', 'row');
  const presetSel = el('select');
  for (const p of SESSION_PRESETS) {
    const o = el('option');
    o.value = p.id;
    o.textContent = p.label;
    presetSel.appendChild(o);
  }
  const pathIn = el('input');
  pathIn.placeholder = '外部目录，如 ~/.claude/projects';
  pathIn.style.minWidth = '18rem';
  const folderIn = el('input');
  folderIn.placeholder = 'agent 名（sessions 下文件夹）';
  folderIn.style.width = '10rem';
  const go = el('button', undefined, '绑定并同步');
  go.type = 'button';
  const hint = el('p', 'muted');
  form.appendChild(el('span', 'muted', '预设'));
  form.appendChild(presetSel);
  form.appendChild(pathIn);
  form.appendChild(folderIn);
  form.appendChild(go);
  card.appendChild(form);
  card.appendChild(hint);

  const applyPreset = () => {
    const p =
      SESSION_PRESETS.find((x) => x.id === presetSel.value) || SESSION_PRESETS[0];
    if (p.id !== 'custom' || !pathIn.value) pathIn.value = p.external;
    if (p.id !== 'custom' || !folderIn.value) folderIn.value = p.folder;
    hint.textContent = p.hint;
  };
  presetSel.onchange = applyPreset;
  applyPreset();

  const listCard = el('div', 'card');
  listCard.appendChild(el('h2', undefined, '已有绑定'));
  const listBar = el('div', 'row');
  const syncAll = el('button', undefined, '全部同步');
  syncAll.type = 'button';
  listBar.appendChild(syncAll);
  listCard.appendChild(listBar);
  const listBox = el('div');
  listBox.style.marginTop = '.5rem';
  listCard.appendChild(listBox);

  go.onclick = async () => {
    setMsg('');
    const external = pathIn.value.trim();
    const folder = folderIn.value.trim();
    if (!external || !folder) {
      setMsg('请填写外部目录和 agent 名');
      return;
    }
    try {
      go.disabled = true;
      const out = await api<{
        binding?: { external?: string };
        report?: BindingReport;
      }>('/admin/api/bindings', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          external,
          space: 'sessions',
          folder,
          mode: 'auto',
          label: `${folder}@admin`,
          sync: true,
        }),
      });
      const r = out.report || {};
      notice(
        `已绑定 ${out.binding?.external || external} → sessions/${folder}` +
          ` · 新增 ${r.added || 0} / 更新 ${r.updated || 0}` +
          (r.errors?.length ? ' · 有错误见列表' : ''),
      );
      await renderBindingsList(listBox);
    } catch (e) {
      setMsg(e);
    } finally {
      go.disabled = false;
    }
  };

  syncAll.onclick = async () => {
    try {
      syncAll.disabled = true;
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
      notice(`同步完成：新增 ${added} / 更新 ${updated}`);
      await renderBindingsList(listBox);
    } catch (e) {
      setMsg(e);
    } finally {
      syncAll.disabled = false;
    }
  };

  app.appendChild(card);
  app.appendChild(listCard);
  await renderBindingsList(listBox);
}
