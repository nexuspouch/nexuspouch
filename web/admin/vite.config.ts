import { resolve } from 'node:path';
import { defineConfig } from 'vite';

const adminRoot = resolve(__dirname);
const sessionsRoot = resolve(adminRoot, 'sessions');

export default defineConfig({
  root: sessionsRoot,
  base: '/admin/sessions/',
  build: {
    outDir: resolve(adminRoot, 'dist/sessions'),
    emptyOutDir: true,
  },
  server: {
    port: 5173,
    proxy: {
      '/admin/api': {
        target: 'http://127.0.0.1:8787',
        changeOrigin: true,
      },
    },
  },
});
