import { resolve } from 'node:path';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

const adminRoot = resolve(__dirname);

export default defineConfig({
  root: resolve(adminRoot, 'sessions'),
  base: '/admin/sessions/',
  plugins: [react()],
  resolve: {
    alias: {
      '@shared': resolve(adminRoot, 'src/shared'),
    },
  },
  build: {
    outDir: resolve(adminRoot, 'dist/sessions'),
    emptyOutDir: true,
  },
  server: {
    port: 5174,
    proxy: {
      '/admin/api': { target: 'http://127.0.0.1:8787', changeOrigin: true },
      '/admin': {
        target: 'http://127.0.0.1:8787',
        changeOrigin: true,
        bypass: (req) =>
          req.url?.startsWith('/admin/sessions') ? req.url : undefined,
      },
    },
  },
});
