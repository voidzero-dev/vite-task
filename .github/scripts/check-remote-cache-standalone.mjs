import { spawn } from 'node:child_process';
import { cp, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, join } from 'node:path';

const pnpm = process.env.npm_execpath;
if (!pnpm) throw new Error('Run this check through pnpm check-remote-cache-standalone');
const directory = await mkdtemp(join(tmpdir(), 'remote-cache-standalone-'));
const excluded = new Set([
  'node_modules',
  '.wrangler',
  'dist',
  'wrangler.operator.json',
  'operator-state.json',
  'benchmark-results.json',
  'e2e-results',
]);
try {
  await cp(new URL('../../packages/remote-cache/', import.meta.url), directory, {
    recursive: true,
    filter(source) {
      const name = basename(source);
      return !excluded.has(name) && !name.startsWith('.dev.vars') && !name.startsWith('.env');
    },
  });
  for (const args of [['install', '--frozen-lockfile'], ['check'], ['build']]) {
    await new Promise((resolve, reject) => {
      const child = spawn(process.execPath, [pnpm, ...args], {
        cwd: directory,
        stdio: 'inherit',
        shell: false,
      });
      child.once('error', reject);
      child.once('exit', (code) =>
        code === 0 ? resolve() : reject(new Error(`Standalone ${args[0]} failed (${code})`)),
      );
    });
  }
} finally {
  await rm(directory, { recursive: true, force: true });
}
