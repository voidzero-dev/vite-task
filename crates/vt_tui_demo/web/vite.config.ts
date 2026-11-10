import { defineConfig } from 'vite-plus';

export default defineConfig({
  server: { port: 5199, strictPort: true },
  build: { target: 'es2022' },
});
