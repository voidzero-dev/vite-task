#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { once } from 'node:events';
import type { AddressInfo } from 'node:net';
import { createCacheServer } from './server.ts';

const basePath = '/projects/test';

/**
 * Emit a milestone for the E2E harness: a window title that
 * `pty_terminal_test` recognizes, in the OSC 2 form it gets on Unix.
 */
function markMilestone(name: string): void {
  const id = randomBytes(16).toString('hex');
  process.stdout.write(
    `\x1b]2;pty-terminal-test:${id}:${Buffer.from(name).toString('base64url')}\x1b\\`,
  );
}

const args = process.argv.slice(2);
const stalledRoutes = new Set<string>();
while (args[0] === '--stall') {
  args.shift();
  const route = args.shift();
  if (route === undefined)
    throw new Error('Usage: remote-cache-server [--stall ROUTE]... COMMAND [ARGS...]');
  stalledRoutes.add(route);
}
const [command, ...commandArgs] = args;
const requests: string[] = [];
const server = createCacheServer({
  basePath,
  directory: 'remote-cache',
  logRequest: (line) => requests.push(line),
  stalledRoutes,
  onStall: () => markMilestone('stalled'),
});
server.listen(0, '127.0.0.1');
await once(server, 'listening');
const { port } = server.address() as AddressInfo;
// Ctrl-C is left to the command.
process.on('SIGINT', () => {});
const child = spawn(command!, commandArgs, {
  stdio: 'inherit',
  env: { ...process.env, VP_REMOTE_CACHE_URL: `http://127.0.0.1:${port}${basePath}` },
});
const [code] = (await once(child, 'exit')) as [number | null];
server.close();
for (const line of requests) console.error(`[remote-cache] ${line}`);
process.exitCode = code;
