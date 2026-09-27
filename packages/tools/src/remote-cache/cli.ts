#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { startCacheServer } from './server.ts';

const [command, ...args] = process.argv.slice(2);
const requests: string[] = [];
const server = await startCacheServer({
  directory: 'remote-cache',
  logRequest: (line) => requests.push(line),
});
const child = spawn(command!, args, {
  stdio: 'inherit',
  env: { ...process.env, VP_REMOTE_CACHE_URL: server.url },
});
const [code] = (await once(child, 'exit')) as [number | null];
await server.close();
for (const line of requests) console.error(`[remote-cache] ${line}`);
process.exitCode = code;
