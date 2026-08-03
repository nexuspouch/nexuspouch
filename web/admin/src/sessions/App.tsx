import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  getToken,
  seedTokenFromQuery,
  setToken,
} from '../shared/api';
import { BindPage } from './components/Bind';
import { DetailPage } from './components/SessionTranscript';
import { OverviewPage } from './components/Overview';
import { SearchPage } from './components/Search';

type Route =
  | { kind: 'overview' }
  | { kind: 'search'; params: URLSearchParams }
  | { kind: 'bind' }
  | { kind: 'detail'; uri: string };

function parseRoute(): Route {
  const h = location.hash || '#/';
  if (h.startsWith('#/s/')) {
    return { kind: 'detail', uri: decodeURIComponent(h.slice(4)) };
  }
  if (h.startsWith('#/search')) {
    const qs = h.includes('?') ? h.slice(h.indexOf('?') + 1) : '';
    return { kind: 'search', params: new URLSearchParams(qs) };
  }
  if (h.startsWith('#/bind')) return { kind: 'bind' };
  return { kind: 'overview' };
}

function navClass(active: boolean): string {
  return active ? 'active' : '';
}

export function App() {
  const [route, setRoute] = useState<Route>(() => parseRoute());
  const [tokenDraft, setTokenDraft] = useState('');
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');

  useEffect(() => {
    seedTokenFromQuery();
    setTokenDraft(getToken());
  }, []);

  useEffect(() => {
    const onHash = () => {
      setNotice('');
      setError('');
      setRoute(parseRoute());
    };
    window.addEventListener('hashchange', onHash);
    return () => window.removeEventListener('hashchange', onHash);
  }, []);

  const onNotice = useCallback((msg: string) => setNotice(msg), []);
  const onError = useCallback((err: unknown) => {
    if (!err) {
      setError('');
      return;
    }
    setError(String((err as Error).message || err));
  }, []);

  const active = useMemo(() => {
    if (route.kind === 'search') return 'search';
    if (route.kind === 'bind') return 'bind';
    if (route.kind === 'detail') return 'overview';
    return 'overview';
  }, [route]);

  return (
    <div className="app-shell">
      <header className="topbar">
        <div>
          <h1>会话管理</h1>
          <p className="sub">
            本机采集 · 统一搜索 · 永不丢失。其它设备用设备过滤，语义回忆见搜索；
            产物 / 记忆空间见 <a href="/admin">管理面</a>。
          </p>
        </div>
        <nav className="nav">
          <a href="#/" className={navClass(active === 'overview')}>
            总览
          </a>
          <a href="#/search" className={navClass(active === 'search')}>
            搜索
          </a>
          <a href="#/bind" className={navClass(active === 'bind')}>
            绑定目录
          </a>
        </nav>
      </header>

      <div className="token-bar">
        <label>
          Token
          <input
            type="password"
            placeholder="admin token"
            value={tokenDraft}
            onChange={(e) => setTokenDraft(e.target.value)}
          />
        </label>
        <button
          type="button"
          onClick={() => {
            setToken(tokenDraft);
            setNotice('Token 已保存');
            setRoute(parseRoute());
          }}
        >
          保存
        </button>
        <span className="hint">与管理面共用</span>
      </div>

      <div className={`banner${notice ? ' show' : ''}`} role="status">
        {notice}
      </div>

      {route.kind === 'overview' && (
        <OverviewPage onNotice={onNotice} onError={onError} />
      )}
      {route.kind === 'search' && (
        <SearchPage
          params={route.params}
          onNotice={onNotice}
          onError={onError}
        />
      )}
      {route.kind === 'bind' && (
        <BindPage onNotice={onNotice} onError={onError} />
      )}
      {route.kind === 'detail' && (
        <DetailPage uri={route.uri} onError={onError} />
      )}

      <p className="err">{error}</p>
    </div>
  );
}
