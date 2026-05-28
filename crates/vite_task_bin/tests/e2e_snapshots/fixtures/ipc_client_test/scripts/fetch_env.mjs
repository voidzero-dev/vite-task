import { getEnv } from '@voidzero-dev/vite-task-client';
import { writeFileSync, mkdirSync } from 'node:fs';

// getEnv returns the env value from the runner and — with tracked: true —
// adds the env to the post-run fingerprint, so a change between runs
// invalidates the cache.
const value = getEnv('PROBE_ENV', { tracked: true }) ?? '(unset)';

mkdirSync('dist', { recursive: true });
writeFileSync('dist/out.txt', 'PROBE_ENV=' + value + '\n');
