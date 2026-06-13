import { getEnv } from '@voidzero-dev/vite-task-client';

const names = process.argv.slice(2);

if (names.length === 0) {
  throw new Error('usage: fetch_env.mjs <NAME> [...]');
}

const served = names.map((name) => `${name}=${getEnv(name) ?? '(unset)'}`);
const own = names.map((name) => `${name}=${process.env[name] ?? '(unset)'}`);

console.log(`served ${served.join(' ')}`);
console.log(`process.env ${own.join(' ')}`);
