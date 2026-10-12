// Reports whether pnpm rewrote its workspace state since `state-after-install.json` was saved.
import { readFileSync } from 'node:fs';

const timestamp = (path) => JSON.parse(readFileSync(path, 'utf8')).lastValidatedTimestamp;
const saved = timestamp('state-after-install.json');
const current = timestamp('node_modules/.pnpm-workspace-state-v1.json');
console.log(
  current > saved ? 'lastValidatedTimestamp increased' : 'lastValidatedTimestamp unchanged',
);
