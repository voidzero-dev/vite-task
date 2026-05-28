import { getEnvs } from '@voidzero-dev/vite-task-client';
import { writeFileSync, mkdirSync } from 'node:fs';

// getEnvs asks the runner for every env matching the glob. The glob (plus
// its match-set) becomes part of the post-run fingerprint, so adding,
// removing, or changing any matching env invalidates the cache on the next
// run. The non-matching UNRELATED envs set by some test steps must not
// contribute.
const matches = getEnvs('PROBE_*', { tracked: true });

mkdirSync('dist', { recursive: true });
const sorted = Object.entries(matches).sort(([a], [b]) => a.localeCompare(b));
const body = sorted.map(([k, v]) => `${k}=${v}`).join('\n');
writeFileSync('dist/out.txt', body + '\n');
