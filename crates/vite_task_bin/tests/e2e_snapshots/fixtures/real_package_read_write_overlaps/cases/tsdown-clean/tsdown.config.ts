import { defineConfig } from 'tsdown';

export default defineConfig({
  clean: true,
  dts: false,
  entry: ['src/index.ts'],
  fixedExtension: false,
  format: 'esm',
  logLevel: 'silent',
  outDir: 'dist',
});
