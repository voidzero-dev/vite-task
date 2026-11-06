import { readFileSync } from 'node:fs';
import { getEnv, getEnvs } from '@voidzero-dev/vite-task-client';

console.log(`TRACKED_ENV=${getEnv('TRACKED_ENV', { tracked: true }) ?? '(unset)'}`);
const matches = getEnvs('PROBE_*', { tracked: true });
for (const [key, value] of Object.entries(matches).sort(([a], [b]) => a.localeCompare(b))) {
  console.log(`${key}=${value}`);
}
console.log(readFileSync('envs_input.txt', 'utf8').trim());
