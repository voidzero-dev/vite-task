#!/usr/bin/env node
import { fork } from 'node:child_process';
import { once } from 'node:events';
import { open, readFile, rm, watch, writeFile } from 'node:fs/promises';
import { createConnection } from 'node:net';
import { Readable } from 'node:stream';
import { setTimeout } from 'node:timers/promises';
import { parseArgs } from 'node:util';
import { createCacheServer } from './server.ts';

const urlFile = 'cache.url';
const logFile = 'cache.log';

async function start(args: string[]): Promise<void> {
  try {
    const file = await open(urlFile, 'wx');
    await file.close();
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'EEXIST') {
      throw new Error('Remote cache server already started (cache.url exists)');
    }
    throw error;
  }
  try {
    const log = await open(logFile, 'w');
    const child = fork(import.meta.filename, ['serve', ...args], {
      detached: true,
      stdio: ['ignore', log.fd, log.fd, 'ipc'],
    });
    try {
      const [message] = await once(child, 'message');
      if (message !== 'ready') throw new Error('Unexpected daemon readiness message');
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      throw new Error(`${message}\n${await readFile(logFile, 'utf8')}`.trim());
    } finally {
      await log.close();
      if (child.connected) child.disconnect();
      child.unref();
    }
  } catch (error) {
    await rm(urlFile, { force: true });
    throw error;
  }
  console.log('Remote cache server started');
}

async function ownsUrlFile(url: string | undefined): Promise<boolean> {
  try {
    return (await readFile(urlFile, 'utf8')).trim() === url;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return false;
    throw error;
  }
}

async function serve(
  maxLifetimeMs: number,
  maxRequestBytes: number,
  basePath: string,
): Promise<void> {
  const server = createCacheServer({ maxRequestBytes, basePath });
  const shutdown = new AbortController();
  const { signal } = shutdown;
  let url: string | undefined;

  async function watchUrlFile(): Promise<void> {
    // Watch the directory so deleting the file is observable on every platform.
    for await (const { filename } of watch('.', { signal })) {
      if ((filename === null || filename === urlFile) && !(await ownsUrlFile(url))) return;
    }
  }

  async function announceReadiness(): Promise<void> {
    // Check after subscribing to file changes, so startup cannot miss a deletion.
    // If the starter disappeared, nobody can use this instance.
    if (!(await ownsUrlFile(url)) || !process.connected || signal.aborted) return;
    process.send?.('ready');
    await setTimeout(maxLifetimeMs, undefined, { signal });
  }

  async function run(): Promise<void> {
    const listening = once(server, 'listening', { signal });
    server.listen(0, '127.0.0.1');
    await listening;
    const address = server.address();
    if (!address || typeof address === 'string') throw new Error('Missing server address');
    url = `http://127.0.0.1:${address.port}${basePath}`;
    // Fill the reserved file before reporting readiness. Do not recreate it if
    // it was deleted during startup.
    await writeFile(urlFile, `${url}\n`, { flag: 'r+' });
    await Promise.race([watchUrlFile(), announceReadiness()]);
  }

  const pending = [
    once(process, 'SIGTERM', { signal }),
    once(process, 'SIGINT', { signal }),
    run(),
  ];
  try {
    await Promise.race(pending);
  } finally {
    shutdown.abort();
    await Promise.allSettled(pending);
    // Finish file cleanup before closing the port, so a subsequent start after
    // `stop` returns cannot have its URL file removed by this instance.
    try {
      if (await ownsUrlFile(url)) await rm(urlFile, { force: true });
    } catch (error) {
      console.error(error);
      process.exitCode = 1;
    } finally {
      if (server.listening) {
        const closed = server[Symbol.asyncDispose]();
        server.closeAllConnections();
        await closed;
      }
    }
  }
}

async function stop(): Promise<void> {
  let endpoint: string;
  try {
    endpoint = await readFile(urlFile, 'utf8');
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
    console.log('No remote cache endpoint found');
    return;
  }
  const url = new URL(endpoint.trim());
  const socket = createConnection({
    host: url.hostname,
    port: Number(url.port),
  });
  const reader = Readable.toWeb(socket).getReader();
  try {
    await once(socket, 'connect');
    await rm(urlFile, { force: true });
    // Shutdown closes the listener before this connection. EOF or a reset
    // confirms the server has stopped accepting connections.
    const { done } = await reader.read();
    if (!done) throw new Error('Unexpected data while waiting for remote cache server shutdown');
  } catch (error) {
    const code = (error as NodeJS.ErrnoException).code;
    if (code !== 'ECONNREFUSED' && code !== 'ECONNRESET') throw error;
    await rm(urlFile, { force: true });
  } finally {
    reader.releaseLock();
    socket.destroy();
  }
  console.log('Remote cache server stopped');
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
      await stop();
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
