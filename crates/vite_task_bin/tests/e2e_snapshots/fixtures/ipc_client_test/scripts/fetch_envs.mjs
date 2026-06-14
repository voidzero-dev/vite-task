import { getEnvs } from '@voidzero-dev/vite-task-client';

const tracked = process.argv[2] === '--untracked' ? false : true;
const matches = getEnvs('PROBE_*', { tracked });
const sorted = Object.entries(matches).sort(([a], [b]) => a.localeCompare(b));

for (const [key, value] of sorted) {
  console.log(`${key}=${value}`);
}
