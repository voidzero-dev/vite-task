import { defineConfig } from 'vite';

export default defineConfig({
  logLevel: 'silent',
  build: {
    rollupOptions: {
      output: {
        // Stable filenames make cache behaviour deterministic across runs.
        entryFileNames: 'assets/main.js',
        chunkFileNames: 'assets/chunk.js',
        assetFileNames: 'assets/[name][extname]',
      },
    },
  },
});
