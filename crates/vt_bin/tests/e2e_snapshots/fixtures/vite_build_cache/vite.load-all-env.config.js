import { defineConfig, loadEnv } from 'vite';

// Reads the whole env the way frameworks such as SvelteKit do, with
// `loadEnv(mode, dir, '')`, and prints what it got for two variables.
export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), '');
  console.log(`APP_LABEL=${env.APP_LABEL ?? '(unset)'} UNRELATED=${env.UNRELATED ?? '(unset)'}`);
  return { logLevel: 'silent' };
});
