#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import type { AddressInfo } from 'node:net';
import { constants } from 'node:os';
import { createCacheServer } from './server.ts';

const basePath = '/projects/test';
const directory = 'remote-cache';

async function main(): Promise<void> {
  const [command, ...args] = process.argv.slice(2);
  if (command === undefined) throw new Error('Usage: remote-cache-server COMMAND [ARGS...]');
  const server = createCacheServer({ basePath, directory });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  try {
    const { port } = server.address() as AddressInfo;
    const child = spawn(command, args, {
      stdio: 'inherit',
      env: { ...process.env, VP_REMOTE_CACHE_URL: `http://127.0.0.1:${port}${basePath}` },
    });
    // The terminal also delivers Ctrl-C to the command. Keep serving until it exits.
    process.on('SIGINT', () => {});
    process.on('SIGTERM', () => child.kill('SIGTERM'));
    const [code, signal] = (await once(child, 'exit')) as [number | null, NodeJS.Signals | null];
    process.exitCode = code ?? 128 + constants.signals[signal!];
  } finally {
    const closed = server[Symbol.asyncDispose]();
    server.closeAllConnections();
    await closed;
  }
}

try {
  await main();
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}
