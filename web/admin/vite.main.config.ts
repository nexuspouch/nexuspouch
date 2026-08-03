import { resolve } from 'node:path';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

const adminRoot = resolve(__dirname);

export default defineConfig({
  root: resolve(adminRoot, 'main'),
  base: '/admin/',
  plugins: [react()],
  resolve: {
    alias: {
      '@shared': resolve(adminRoot, 'src/shared'),
    },
  },
  build: {
    outDir: resolve(adminRoot, 'dist/main'),
    emptyOutDir: true,
  },
  server: {
    port: 5173,
    proxy: {
      '/admin/api': { target: 'http://127.0.0.1:8787', changeOrigin: true },
      '/admin/sessions': { target: 'http://127.0.0.1:8787', changeOrigin: true },
    },
  },
});
