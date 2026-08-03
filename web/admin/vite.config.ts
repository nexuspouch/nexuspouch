import { resolve } from 'node:path';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

const adminRoot = resolve(__dirname);

/** 单入口 MPA：/admin/（主管理面）+ /admin/sessions/（会话） */
export default defineConfig({
  root: adminRoot,
  base: '/admin/',
  plugins: [react()],
  resolve: {
    alias: {
      '@shared': resolve(adminRoot, 'src/shared'),
    },
  },
  build: {
    outDir: resolve(adminRoot, 'dist'),
    emptyOutDir: true,
    rollupOptions: {
      input: {
        main: resolve(adminRoot, 'index.html'),
        sessions: resolve(adminRoot, 'sessions/index.html'),
      },
    },
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
