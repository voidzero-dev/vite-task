import { defineConfig } from 'nitro';

export default defineConfig({
  serverDir: './',
  // Trace these packages instead of bundling them. They depend on different
  // versions of `tslib`, so nf3 stores each version under `node_modules/.nf3`
  // and links to it.
  traceDeps: ['dep-a', 'dep-b'],
  // Keep the build output stable for the snapshot: no date-dependent setting,
  // no build log (`CONSOLA_LEVEL=0` in the task command quiets nf3's), and no
  // bundler warnings, which Nitro prints itself.
  compatibilityDate: '2026-10-29',
  logLevel: 0,
  rolldownConfig: { onwarn() {} },
});
