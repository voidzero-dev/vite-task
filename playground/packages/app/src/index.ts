/// <reference types="vite/client" />

import { sum } from 'lib';

// `import.meta.env.VITE_MODE` is substituted at build time from the value
// vite's patched `loadEnv` fetched via the runner. Dead-code elimination
// leaves only the branch matching whatever `VITE_MODE=...` was set on the
// run, so the bundle reflects (and the runner tracks) that env.
if (import.meta.env.VITE_MODE === 'production') {
  console.log('PROD build:', sum(1, 2, 3));
} else {
  console.log('DEV build:', sum(1, 2, 3));
}
