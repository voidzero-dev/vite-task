#!/usr/bin/env node
import { fork } from 'node:child_process';
import { mkdir, open, readFile, rename, rm, stat, writeFile } from 'node:fs/promises';
import { setTimeout as delay } from 'node:timers/promises';
import { parseArgs } from 'node:util';
import { createCacheServer } from './server.ts';

const urlFile = 'cache.url';
const lockDir = 'cache.lock';
const logFile = 'cache.log';
const startupTimeout = 10_000;

async function exists(path: string): Promise<boolean> {
  try {
    await stat(path);
    return true;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return false;
    throw error;
  }
}

async function waitUntil(check: () => Promise<boolean>): Promise<void> {
  const deadline = Date.now() + startupTimeout;
  while (!(await check())) {
    if (Date.now() >= deadline) throw new Error('Timed out waiting for remote cache server');
    await delay(50);
  }
}

async function start(args: string[]): Promise<void> {
  try {
    await mkdir(lockDir);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'EEXIST') {
      throw new Error('Remote cache server already started (cache.lock exists)');
    }
    throw error;
  }
  try {
    if (await exists(urlFile)) throw new Error('cache.url already exists');
    const log = await open(logFile, 'w');
    const child = fork(import.meta.filename, ['serve', ...args], {
      detached: true,
      stdio: ['ignore', log.fd, log.fd, 'ipc'],
    });
    try {
      await new Promise<void>((resolve, reject) => {
        const timeout = setTimeout(
          () => reject(new Error('Remote cache server startup timed out')),
          startupTimeout,
        );
        child.once('message', (message) => {
          clearTimeout(timeout);
          if (message === 'ready') resolve();
          else reject(new Error('Unexpected daemon readiness message'));
        });
        child.once('error', (error) => {
          clearTimeout(timeout);
          reject(error);
        });
        child.once('exit', () => {
          clearTimeout(timeout);
          reject(new Error('Remote cache server exited before readiness'));
        });
      });
    } catch (error) {
      // A failed startup must not leave a detached child or a stale lock.
      if (child.pid !== undefined && child.exitCode === null && child.signalCode === null) {
        const exited = new Promise<void>((resolve) => child.once('exit', () => resolve()));
        child.kill('SIGKILL');
        await exited;
      }
      await rm(urlFile, { force: true });
      throw new Error(
        `${error instanceof Error ? error.message : String(error)}\n${await readFile(logFile, 'utf8')}`.trim(),
      );
    } finally {
      await log.close();
      if (child.connected) child.disconnect();
      child.unref();
    }
  } catch (error) {
    await rm(lockDir, { recursive: true, force: true });
    throw error;
  }
  console.log('Remote cache server started');
}

async function serve(
  maxLifetimeMs: number,
  maxRequestBytes: number,
  basePath: string,
): Promise<void> {
  const server = createCacheServer({ maxRequestBytes, basePath });
  let stopping = false;
  let ready = false;
  let url: string | undefined;
  process.on('SIGTERM', () => {
    stopping = true;
  });
  process.on('SIGINT', () => {
    stopping = true;
  });
  // If the starter disappears before readiness, nobody can use this instance.
  process.on('disconnect', () => {
    if (!ready) stopping = true;
  });
  try {
    await new Promise<void>((resolve, reject) => {
      server.once('error', reject);
      server.listen(0, '127.0.0.1', resolve);
    });
    const address = server.address();
    if (!address || typeof address === 'string') throw new Error('Missing server address');
    url = `http://127.0.0.1:${address.port}${basePath}`;
    await writeFile(`${lockDir}/url.tmp`, `${url}\n`);
    await rename(`${lockDir}/url.tmp`, urlFile);
    if (process.connected) process.send?.('ready');
    ready = true;
    const deadline = Date.now() + maxLifetimeMs;
    while (!stopping && Date.now() < deadline) {
      try {
        if ((await readFile(urlFile, 'utf8')).trim() !== url) break;
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code === 'ENOENT') break;
        throw error;
      }
      await delay(50);
    }
  } finally {
    await new Promise<void>((resolve) => {
      server.close(() => resolve());
      server.closeAllConnections();
    });
    // Removing the lock acknowledges that the listening socket has closed.
    try {
      if ((await readFile(urlFile, 'utf8')).trim() === url) await rm(urlFile, { force: true });
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') {
        console.error(error);
        process.exitCode = 1;
      }
    } finally {
      await rm(lockDir, { recursive: true, force: true });
    }
  }
}

function positiveInteger(value: string): number {
  const number = Number(value);
  if (!Number.isSafeInteger(number) || number <= 0 || number > 2_147_483_647) {
    throw new Error('Limits must be positive integers no greater than 2147483647');
  }
  return number;
}

async function main(): Promise<void> {
  const { values, positionals } = parseArgs({
    allowPositionals: true,
    options: {
      'max-lifetime-ms': { type: 'string', default: '300000' },
      'max-request-bytes': { type: 'string', default: String(64 * 1024 * 1024) },
      'base-path': { type: 'string', default: '' },
    },
  });
  const lifetime = positiveInteger(values['max-lifetime-ms']);
  const limit = positiveInteger(values['max-request-bytes']);
  const basePath = values['base-path'];
  if (basePath && !/^\/(?:[a-zA-Z0-9_-]+\/)*[a-zA-Z0-9_-]+$/.test(basePath)) {
    throw new Error('Base path must contain slash-separated names without a trailing slash');
  }
  if (positionals.length !== 1) throw new Error('Usage: remote-cache-server start|stop');
  switch (positionals[0]) {
    case 'start':
      await start([
        '--max-lifetime-ms',
        String(lifetime),
        '--max-request-bytes',
        String(limit),
        '--base-path',
        basePath,
      ]);
      break;
    case 'serve':
      await serve(lifetime, limit, basePath);
      break;
    case 'stop':
      await rm(urlFile, { force: true });
      await waitUntil(async () => !(await exists(lockDir)));
      console.log('Remote cache server stopped');
      break;
    default:
      throw new Error('Usage: remote-cache-server start|stop');
  }
}

try {
  await main();
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}
