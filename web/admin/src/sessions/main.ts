import './styles.css';
import {
  getToken,
  seedTokenFromQuery,
  setToken,
} from '../shared/api';
import { $, clearNotice, setMsg } from '../shared/dom';
import { showBind } from './pages/bind';
import { showDetail } from './pages/detail';
import { showOverview } from './pages/overview';
import { showSearch } from './pages/search';

function initTokenBar(): void {
  seedTokenFromQuery();
  const input = $('token') as HTMLInputElement;
  input.value = getToken();
  $('saveToken').onclick = () => {
    setToken(input.value);
    void route();
  };
}

async function route(): Promise<void> {
  setMsg('');
  clearNotice();
  const h = location.hash || '#/';
  if (h.startsWith('#/s/')) {
    await showDetail(decodeURIComponent(h.slice(4)));
  } else if (h.startsWith('#/search')) {
    const qs = h.includes('?') ? h.slice(h.indexOf('?') + 1) : '';
    await showSearch(new URLSearchParams(qs));
  } else if (h.startsWith('#/bind')) {
    await showBind();
  } else {
    await showOverview();
  }
}

initTokenBar();
window.addEventListener('hashchange', () => void route());
void route();
