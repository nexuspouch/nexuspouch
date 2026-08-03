import { api } from '../../shared/api';
import { clear, el, esc, $ } from '../../shared/dom';
import { fmtBytes, fmtTime, shortId, toolName } from '../../shared/format';
import type { SessionDetail } from '../types';

export async function showDetail(uri: string): Promise<void> {
  const app = $('app');
  clear(app);

  const back = el('p');
  const backA = el('a', undefined, '← 返回总览');
  backA.href = '#/';
  back.appendChild(backA);
  app.appendChild(back);

  const card = el('div', 'card');
  card.appendChild(el('p', 'muted', '加载中…'));
  app.appendChild(card);

  try {
    const d = await api<SessionDetail>(
      `/admin/api/sessions/detail?uri=${encodeURIComponent(uri)}`,
    );
    clear(card);

    const head = el('div', 'row');
    head.style.justifyContent = 'space-between';
    const titleBox = el('div');
    titleBox.appendChild(
      el('h2', undefined, `${d.agent || ''} · ${d.session_id || ''}`),
    );
    titleBox.appendChild(
      el(
        'div',
        'muted',
        `${d.project ? `${d.project} · ` : ''}设备 ${shortId(d.device || '')}` +
          ` · ${d.kind || ''} · ${fmtBytes(d.size || 0)} · 原文（未脱敏）`,
      ),
    );
    head.appendChild(titleBox);

    const vbox = el('label', 'row');
    vbox.appendChild(el('span', 'muted', '版本'));
    const vsel = el('select');
    const versions = d.versions || [];
    for (const v of versions) {
      const o = el('option');
      o.value = `v${v.v}`;
      o.textContent = `v${v.v} · ${fmtTime(v.mtime)} · ${fmtBytes(v.size)}`;
      vsel.appendChild(o);
    }
    vbox.appendChild(vsel);
    if (d.protected) vbox.appendChild(el('span', 'badge', 'protected'));
    if (versions.length) {
      const top = versions[versions.length - 1];
      const cur =
        d.ref === 'latest'
          ? `v${d.version != null ? d.version : top.v}`
          : d.ref || '';
      vsel.value = cur;
      vsel.onchange = () => {
        const pick = vsel.value;
        const target =
          `v${top.v}` === pick ? d.uri : `${d.uri}@${pick}`;
        location.hash = `#/s/${encodeURIComponent(target)}`;
      };
    }
    head.appendChild(vbox);
    card.appendChild(head);

    if (d.truncated) {
      card.appendChild(el('p', 'muted', '（内容过长，已截断显示）'));
    }

    const flow = el('div');
    for (const t of d.turns || []) {
      if (t.role === 'tool') {
        const det = el('details', 'tool');
        det.appendChild(el('summary', undefined, `🔧 ${toolName(t.content || '')}`));
        det.appendChild(el('pre', undefined, t.content || ''));
        flow.appendChild(det);
        continue;
      }
      const div = el(
        'div',
        `turn ${t.role === 'user' ? 'user' : 'assistant'}`,
      );
      div.appendChild(
        el(
          'div',
          'meta',
          `${t.role === 'user' ? '用户' : '助手'}${t.ts_ms ? ` · ${fmtTime(t.ts_ms)}` : ''}`,
        ),
      );
      div.appendChild(el('div', undefined, t.content || ''));
      flow.appendChild(div);
    }
    if (!(d.turns || []).length) {
      flow.appendChild(
        el('p', 'muted', '（无法解析出对话内容——文件可能为空或格式未识别）'),
      );
    }
    card.appendChild(flow);
    $('navHint').textContent = '';
  } catch (e) {
    clear(card);
    card.appendChild(el('p', 'err', esc(String((e as Error).message || e))));
  }
}
