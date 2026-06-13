import { getEnvs } from '@voidzero-dev/vite-task-client';

const matches = getEnvs('PROBE_*');
const sorted = Object.entries(matches).sort(([a], [b]) => a.localeCompare(b));

for (const [key, value] of sorted) {
  console.log(`${key}=${value}`);
}
