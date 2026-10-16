import { getEnvs } from '@voidzero-dev/vite-task-client';

const args = process.argv.slice(2);
const tracked = !args.includes('--untracked');
const prefixIndex = args.indexOf('--prefix');
const globIndex = args.indexOf('--glob');
const glob = globIndex === -1 ? 'PROBE_*' : (args[globIndex + 1] ?? 'PROBE_*');
const query = prefixIndex === -1 ? glob : { prefix: args[prefixIndex + 1] ?? 'PROBE_' };
const matches = getEnvs(query, { tracked });
const printPrefixIndex = args.indexOf('--print-prefix');
const printPrefix = printPrefixIndex === -1 ? '' : (args[printPrefixIndex + 1] ?? '');
const sorted = Object.entries(matches)
  .filter(([name]) => name.startsWith(printPrefix))
  .sort(([a], [b]) => a.localeCompare(b));

for (const [key, value] of sorted) {
  console.log(`${key}=${value}`);
}
