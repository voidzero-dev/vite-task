#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import type { AddressInfo } from 'node:net';
import { createCacheServer } from './server.ts';

const basePath = '/projects/test';

const [command, ...args] = process.argv.slice(2);
const requests: string[] = [];
const server = createCacheServer({
  basePath,
  directory: 'remote-cache',
  logRequest: (line) => requests.push(line),
});
server.listen(0, '127.0.0.1');
await once(server, 'listening');
const { port } = server.address() as AddressInfo;
const child = spawn(command!, args, {
  stdio: 'inherit',
  env: { ...process.env, VP_REMOTE_CACHE_URL: `http://127.0.0.1:${port}${basePath}` },
});
const [code] = (await once(child, 'exit')) as [number | null];
server.close();
for (const line of requests) console.error(`[remote-cache] ${line}`);
process.exitCode = code;
