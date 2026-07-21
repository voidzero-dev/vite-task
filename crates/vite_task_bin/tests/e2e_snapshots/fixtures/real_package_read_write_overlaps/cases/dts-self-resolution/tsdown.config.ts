import { defineConfig } from 'tsdown';

export default defineConfig({
  clean: false,
  dts: { emitDtsOnly: true },
  entry: ['src/index.ts'],
  fixedExtension: false,
  format: 'esm',
  logLevel: 'silent',
  outDir: 'dist',
});
