#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { closeSync, existsSync, mkdirSync, openSync, readFileSync } from 'node:fs';
import { request } from 'node:http';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { logFile, serverFile, stateDirectory, type ServerInfo } from './state.ts';

const usage = 'Usage: remote-cache-server start | run COMMAND [ARGS...] | stop';
const directory = resolve(stateDirectory);

function fail(message: string): never {
  console.error(`remote-cache-server: ${message}`);
  process.exit(1);
}

function readInfo(): ServerInfo {
  if (!existsSync(serverFile(directory))) {
    fail('no backend is running here. Start one with `remote-cache-server start`.');
  }
  return JSON.parse(readFileSync(serverFile(directory), 'utf8')) as ServerInfo;
}

/** POST to the backend's control server and return the JSON response. */
function control<T>(info: ServerInfo, path: string): Promise<T> {
  return new Promise((resolve) => {
    const unreachable = (error: Error) =>
      fail(`the backend is unreachable (${error.message}). See ${logFile(stateDirectory)}.`);
    const call = request(`${info.control}${path}`, { method: 'POST', agent: false }, (response) => {
      const chunks: Buffer[] = [];
      response.on('data', (chunk: Buffer) => chunks.push(chunk));
      response.on('end', () => resolve(JSON.parse(Buffer.concat(chunks).toString()) as T));
      response.on('error', unreachable);
    });
    call.on('error', unreachable);
    call.end();
  });
}

function printRequests(lines: string[]): void {
  for (const line of lines) console.error(`[remote-cache] ${line}`);
}

async function start(): Promise<void> {
  if (existsSync(serverFile(directory))) fail('a backend is already running here.');
  mkdirSync(directory, { recursive: true });
  const log = openSync(logFile(directory), 'a');
  // The backend outlives this step, so it gets its own session and none of the
  // terminal's file descriptors. The terminal then closes when this step
  // exits, and Ctrl-C in later steps doesn't reach the backend.
  const daemon = spawn(
    process.execPath,
    [...process.execArgv, fileURLToPath(new URL('daemon.ts', import.meta.url)), directory],
    { cwd: tmpdir(), detached: true, stdio: ['ignore', log, log, 'ipc'] },
  );
  closeSync(log);
  const ready = await Promise.race([
    once(daemon, 'message').then(() => true),
    once(daemon, 'exit').then(() => false),
  ]);
  if (!ready) fail(`the backend failed to start:\n${readFileSync(logFile(directory), 'utf8')}`);
  daemon.disconnect();
  daemon.unref();
}

async function run([command, ...args]: string[]): Promise<void> {
  if (command === undefined) fail(usage);
  const info = readInfo();
  // Ctrl-C is left to the command.
  process.on('SIGINT', () => {});
  const child = spawn(command, args, {
    stdio: 'inherit',
    env: { ...process.env, VP_REMOTE_CACHE_URL: info.url },
  });
  const [code] = (await once(child, 'exit')) as [number | null];
  printRequests(await control<string[]>(info, '/take'));
  process.exitCode = code;
}

async function stop(): Promise<void> {
  const { requests, anomalies } = await control<{ requests: string[]; anomalies: string[] }>(
    readInfo(),
    '/stop',
  );
  printRequests(requests);
  for (const anomaly of anomalies) console.error(`remote-cache-server: ${anomaly}`);
  if (anomalies.length > 0) process.exitCode = 1;
}

const [subcommand, ...args] = process.argv.slice(2);
if (subcommand === 'start') await start();
else if (subcommand === 'run') await run(args);
else if (subcommand === 'stop') await stop();
else fail(usage);
