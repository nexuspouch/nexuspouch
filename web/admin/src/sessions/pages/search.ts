import { api, postFeedback } from '../../shared/api';
import { clear, el, esc, notice, setMsg, $ } from '../../shared/dom';
import { shortId } from '../../shared/format';
import type { SearchHit } from '../types';

export async function showSearch(params: URLSearchParams): Promise<void> {
  const app = $('app');
  clear(app);

  const card = el('div', 'card');
  card.appendChild(el('h2', undefined, '搜索会话'));
  const form = el('form', 'row');
  const q = el('input');
  q.placeholder = '关键词或语义描述，如「上次部署报错怎么解决」';
  q.style.minWidth = '18rem';
  q.value = params.get('q') || '';

  const agentIn = el('input');
  agentIn.placeholder = 'agent（可选）';
  agentIn.style.width = '8rem';
  agentIn.value = params.get('agent') || '';

  const projectIn = el('input');
  projectIn.placeholder = 'project（可选）';
  projectIn.style.width = '8rem';
  projectIn.value = params.get('project') || '';

  const sinceIn = el('input');
  sinceIn.type = 'date';
  sinceIn.title = 'since（可选）';
  if (params.get('since_ms')) {
    const d = new Date(Number(params.get('since_ms')));
    if (!Number.isNaN(d.getTime())) sinceIn.value = d.toISOString().slice(0, 10);
  }

  const untilIn = el('input');
  untilIn.type = 'date';
  untilIn.title = 'until（可选）';
  if (params.get('until_ms')) {
    const d = new Date(Number(params.get('until_ms')));
    if (!Number.isNaN(d.getTime())) untilIn.value = d.toISOString().slice(0, 10);
  }

  const devSel = el('select');
  const semLabel = el('label', 'row');
  const sem = el('input');
  sem.type = 'checkbox';
  sem.checked = params.get('semantic') === 'true';
  semLabel.appendChild(sem);
  semLabel.appendChild(el('span', undefined, '语义回忆（hybrid）'));

  const dedupLabel = el('label', 'row');
  const dedup = el('input');
  dedup.type = 'checkbox';
  dedup.checked = params.get('dedup') !== 'false';
  dedupLabel.appendChild(dedup);
  dedupLabel.appendChild(el('span', undefined, '按项目去重'));

  const go = el('button', undefined, '搜索');
  go.type = 'submit';
  form.appendChild(q);
  form.appendChild(agentIn);
  form.appendChild(projectIn);
  form.appendChild(sinceIn);
  form.appendChild(untilIn);
  form.appendChild(devSel);
  form.appendChild(semLabel);
  form.appendChild(dedupLabel);
  form.appendChild(go);
  card.appendChild(form);

  const results = el('div');
  card.appendChild(results);
  app.appendChild(card);

  try {
    const ov = await api<{ devices?: string[] }>('/admin/api/sessions/overview');
    devSel.replaceChildren();
    const all = el('option');
    all.value = '';
    all.textContent = '全部设备';
    devSel.appendChild(all);
    for (const d of ov.devices || []) {
      const o = el('option');
      o.value = d;
      o.textContent = shortId(d);
      devSel.appendChild(o);
    }
    const want = params.get('device') || '';
    if (want) devSel.value = want;
  } catch {
    /* optional filter */
  }

  form.onsubmit = (ev) => {
    ev.preventDefault();
    const p = new URLSearchParams();
    if (q.value.trim()) p.set('q', q.value.trim());
    if (agentIn.value.trim()) p.set('agent', agentIn.value.trim());
    if (projectIn.value.trim()) p.set('project', projectIn.value.trim());
    if (sinceIn.value)
      p.set('since_ms', String(Date.parse(`${sinceIn.value}T00:00:00Z`)));
    if (untilIn.value)
      p.set('until_ms', String(Date.parse(`${untilIn.value}T23:59:59Z`)));
    if (sem.checked) p.set('semantic', 'true');
    if (!dedup.checked) p.set('dedup', 'false');
    if (devSel.value) p.set('device', devSel.value);
    location.hash = `#/search?${p.toString()}`;
  };

  const query = (params.get('q') || '').trim();
  if (!query) {
    results.appendChild(
      el(
        'p',
        'muted',
        '输入查询开始。可按 agent / project / 日期过滤；勾选「语义回忆」走 FTS5+向量混合排序；默认按项目去重并附带片段上下文。',
      ),
    );
    return;
  }

  try {
    const p = new URLSearchParams({ q: query });
    if (params.get('semantic') === 'true') p.set('semantic', 'true');
    if (params.get('dedup') === 'false') p.set('dedup', 'false');
    if (params.get('device')) p.set('device', params.get('device')!);
    if (params.get('agent')) p.set('agent', params.get('agent')!);
    if (params.get('project')) p.set('project', params.get('project')!);
    if (params.get('since_ms')) p.set('since_ms', params.get('since_ms')!);
    if (params.get('until_ms')) p.set('until_ms', params.get('until_ms')!);

    const out = await api<{
      total?: number;
      score_type?: string;
      rerank?: boolean;
      dedup?: boolean;
      embedder?: string;
      degraded?: boolean;
      results?: SearchHit[];
    }>(`/admin/api/sessions/search?${p.toString()}`);

    clear(results);
    results.appendChild(
      el(
        'p',
        'muted',
        `命中 ${out.total || 0} 条 · ${out.score_type || 'keyword'}` +
          (out.rerank ? ' · rerank' : '') +
          (out.dedup ? ' · dedup' : '') +
          (out.embedder ? ` · ${out.embedder}` : ''),
      ),
    );
    if (out.degraded) {
      notice(
        '向量检索不可用，已回退关键词匹配（语义回忆需要本地 embedding 模型）。',
      );
    }

    const hits = out.results || [];
    if (!hits.length) {
      results.appendChild(
        el('p', 'muted', '没有命中。换个说法试试，或先确认会话已入库。'),
      );
      return;
    }

    hits.forEach((h, i) => {
      const box = el('div', 'hit');
      const head = el('div');
      const link = el('a', 'title-link', h.path || h.uri);
      link.href = `#/s/${encodeURIComponent(h.uri)}`;
      link.addEventListener('click', () => {
        void postFeedback(query, h, i + 1, 'click').catch(() => {});
      });
      head.appendChild(link);
      head.appendChild(el('span', 'muted', ` · 设备 ${shortId(h.device || '')}`));
      if (h.score_type) head.appendChild(el('span', 'badge', h.score_type));
      if (h.occurrence_label)
        head.appendChild(el('span', 'badge', h.occurrence_label));
      else if (h.occurrences && h.occurrences > 1)
        head.appendChild(el('span', 'badge', `${h.occurrences}×`));
      box.appendChild(head);

      const snip = el('div', 'snip');
      snip.innerHTML = esc(h.snippet || '')
        .replaceAll('&lt;b&gt;', '<b>')
        .replaceAll('&lt;/b&gt;', '</b>');
      box.appendChild(snip);

      if (h.fragment?.turns?.length) {
        const frag = el('div', 'snip');
        frag.style.opacity = '0.85';
        frag.textContent = h.fragment.turns
          .map(
            (t) =>
              `${t.focus ? '▶ ' : '  '}${t.role || ''}: ${String(t.content || '').slice(0, 160)}`,
          )
          .join('\n');
        box.appendChild(frag);
      }

      const fb = el('div', 'row');
      fb.style.marginTop = '0.35rem';
      const mkBtn = (label: string, kind: string) => {
        const b = el('button', undefined, label);
        b.type = 'button';
        b.style.fontSize = '0.8rem';
        b.onclick = async () => {
          try {
            await postFeedback(query, h, i + 1, kind);
            notice(kind === 'wrong' ? '已记录：结果不对' : '已记录：有用');
          } catch (e) {
            setMsg(e);
          }
        };
        return b;
      };
      fb.appendChild(mkBtn('有用', 'adopt'));
      fb.appendChild(mkBtn('不对', 'wrong'));
      box.appendChild(fb);
      results.appendChild(box);
    });
  } catch (e) {
    setMsg(e);
  }
}
