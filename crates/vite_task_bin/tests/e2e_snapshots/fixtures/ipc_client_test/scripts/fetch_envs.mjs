import { getEnvs } from '@voidzero-dev/vite-task-client';

const args = process.argv.slice(2);
const tracked = !args.includes('--untracked');
const prefixIndex = args.indexOf('--prefix');
const query = prefixIndex === -1 ? 'PROBE_*' : { prefix: args[prefixIndex + 1] ?? 'PROBE_' };
const matches = getEnvs(query, { tracked });
const sorted = Object.entries(matches).sort(([a], [b]) => a.localeCompare(b));

for (const [key, value] of sorted) {
  console.log(`${key}=${value}`);
}
