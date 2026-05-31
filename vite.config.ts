import { defineConfig } from 'vite-plus';

export default defineConfig({
  staged: {
    '*': 'vp check --fix',
    '*.rs': 'cargo fmt --',
  },
  lint: {
    jsPlugins: [{ name: 'vite-plus', specifier: 'vite-plus/oxlint-plugin' }],
    rules: { 'vite-plus/prefer-vite-plus-imports': 'error' },
    options: { typeAware: true, typeCheck: true },
    ignorePatterns: ['playground/**', '**/fixtures/**'],
  },
  fmt: {
    singleQuote: true,
    ignorePatterns: [
      'crates/fspy_detours_sys/detours',
      'crates/vite_task_graph/run-config.ts',
      '**/fixtures/*/snapshots',
      'packages/vite-task-client/src/index.d.ts',
    ],
  },
});
