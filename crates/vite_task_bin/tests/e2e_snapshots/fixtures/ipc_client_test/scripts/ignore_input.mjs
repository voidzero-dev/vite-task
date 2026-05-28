import { ignoreInput } from '@voidzero-dev/vite-task-client';
import { writeFileSync, readFileSync } from 'node:fs';
import { mkdirSync } from 'node:fs';

// The task reads from `cache_like/` (which we want the runner to IGNORE as
// an input), and writes to `dist/`. Without the ignore, the auto-input
// fingerprint would fluctuate with cache_like/ contents even though they're
// not semantic inputs.
mkdirSync('cache_like', { recursive: true });
writeFileSync('cache_like/stale.txt', 'stale-' + Date.now() + '\n');
ignoreInput('cache_like');
readFileSync('cache_like/stale.txt', 'utf8');

mkdirSync('dist', { recursive: true });
writeFileSync('dist/out.txt', 'ok\n');
