import { getEnv } from '@voidzero-dev/vite-task-client';

const args = process.argv.slice(2);
const tracked = args[0] === '--untracked' ? false : true;
const names = tracked ? args : args.slice(1);

if (names.length === 0) {
  throw new Error('usage: fetch_env.mjs [--untracked] <NAME> [...]');
}

const served = names.map((name) => `${name}=${getEnv(name, { tracked }) ?? '(unset)'}`);
const own = names.map((name) => `${name}=${process.env[name] ?? '(unset)'}`);

console.log(`served ${served.join(' ')}`);
console.log(`process.env ${own.join(' ')}`);
