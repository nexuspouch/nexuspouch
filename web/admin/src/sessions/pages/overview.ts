import { api } from '../../shared/api';
import { clear, el, setMsg, $ } from '../../shared/dom';
import { fmtBytes, fmtDuration, fmtTime, shortId } from '../../shared/format';
import type { SessionRow } from '../types';

export async function showOverview(device?: string): Promise<void> {
  const app = $('app');
  clear(app);

  const card = el('div', 'card');
  const bar = el('div', 'row');
  bar.appendChild(el('h2', undefined, '全部会话'));
  const devSel = el('select');
  bar.appendChild(el('span', 'muted', '设备'));
  bar.appendChild(devSel);
  const refreshBtn = el('button', undefined, '刷新');
  refreshBtn.type = 'button';
  bar.appendChild(refreshBtn);
  const count = el('span', 'muted');
  bar.appendChild(count);
  card.appendChild(bar);

  const body = el('div');
  body.style.marginTop = '.75rem';
  card.appendChild(body);
  app.appendChild(card);

  refreshBtn.onclick = () => void showOverview(devSel.value || undefined);
  devSel.onchange = () => void showOverview(devSel.value || undefined);

  try {
    const q = new URLSearchParams();
    if (device) q.set('device', device);
    const out = await api<{
      devices?: string[];
      total?: number;
      truncated?: boolean;
      sessions?: SessionRow[];
    }>('/admin/api/sessions/overview' + (q.size ? `?${q}` : ''));

    devSel.replaceChildren();
    const all = el('option');
    all.value = '';
    all.textContent = '全部设备';
    devSel.appendChild(all);
    for (const d of out.devices || []) {
      const o = el('option');
      o.value = d;
      o.textContent = shortId(d);
      devSel.appendChild(o);
    }
    if (device) devSel.value = device;

    count.textContent =
      `共 ${out.total || 0} 个会话` +
      (out.truncated
        ? `（仅显示前 ${(out.sessions || []).length} 个，可加设备过滤）`
        : '');

    const sessions = out.sessions || [];
    if (!sessions.length) {
      const empty = el('p', 'muted');
      empty.appendChild(document.createTextNode('暂无会话。请先 '));
      const bindLink = el('a', undefined, '绑定 agent 会话目录');
      bindLink.href = '#/bind';
      empty.appendChild(bindLink);
      empty.appendChild(
        document.createTextNode('（或经 agent-bridge 旁路写入），同步后这里会出现。'),
      );
      body.appendChild(empty);
      return;
    }

    const groups: Record<string, SessionRow[]> = {};
    const order: string[] = [];
    for (const s of sessions) {
      const a = s.agent || 'unknown';
      if (!groups[a]) {
        groups[a] = [];
        order.push(a);
      }
      groups[a].push(s);
    }

    for (const a of order) {
      const list = groups[a];
      body.appendChild(el('h3', undefined, `${a}（${list.length}）`));
      const table = el('table');
      table.innerHTML =
        '<thead><tr><th>会话</th><th>项目</th><th>更新时间</th>' +
        '<th>时长</th><th>事件数</th><th>大小</th></tr></thead>';
      const tbody = el('tbody');
      for (const s of list) {
        const tr = el('tr');
        const tdT = el('td');
        const link = el('a', 'title-link', s.title || s.session_id || s.uri);
        link.href = `#/s/${encodeURIComponent(s.uri)}`;
        tdT.appendChild(link);
        tdT.appendChild(
          el('div', 'muted', `${s.session_id || ''} · ${s.kind || ''}`),
        );
        tr.appendChild(tdT);
        tr.appendChild(el('td', undefined, s.project || '—'));
        tr.appendChild(el('td', undefined, fmtTime(s.mtime || 0)));
        tr.appendChild(el('td', undefined, fmtDuration(s.duration_ms)));
        tr.appendChild(el('td', undefined, String(s.events || 0)));
        tr.appendChild(el('td', undefined, fmtBytes(s.size || 0)));
        tbody.appendChild(tr);
      }
      table.appendChild(tbody);
      body.appendChild(table);
    }
    $('navHint').textContent = '';
  } catch (e) {
    setMsg(e);
  }
}
