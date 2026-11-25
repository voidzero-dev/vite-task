#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { closeSync, existsSync, mkdirSync, openSync, readFileSync } from 'node:fs';
import { request } from 'node:http';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import crossSpawn from 'cross-spawn';
import { defaultRequestToken } from './github.ts';
import {
  logFile,
  serverFile,
  stateDirectory,
  type FaultKind,
  type FaultRoute,
  type FaultRule,
  type RunConfig,
  type ServerInfo,
} from './state.ts';

const usage =
  'Usage: remote-cache-server start [--entries-per-key N] | run [--github-actions] [--fault ROUTE=KIND]... [--always-fail ROUTE=KIND]... [--hold-stores N] [--release-after-fetches N] [--summary] COMMAND [ARGS...] | corrupt-blob NUMBER | expire-entry [--before-download] NUMBER | stop';
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

/**
 * POST to the backend's control server and return the JSON response. A 404
 * fails with `notFound`.
 */
function control<T>(
  info: ServerInfo,
  path: string,
  notFound = 'unknown request',
  body?: unknown,
): Promise<T> {
  return new Promise((resolve) => {
    const unreachable = (error: Error) =>
      fail(`the backend is unreachable (${error.message}). See ${logFile(stateDirectory)}.`);
    const payload = body === undefined ? undefined : Buffer.from(JSON.stringify(body));
    const headers =
      payload === undefined
        ? undefined
        : { 'content-type': 'application/json', 'content-length': String(payload.length) };
    const call = request(
      `${info.control}${path}`,
      { method: 'POST', agent: false, ...(headers === undefined ? {} : { headers }) },
      (response) => {
        if (response.statusCode === 404) fail(notFound);
        if (response.statusCode !== 200) {
          fail(`the backend failed (${response.statusCode}). See ${logFile(stateDirectory)}.`);
        }
        const chunks: Buffer[] = [];
        response.on('data', (chunk: Buffer) => chunks.push(chunk));
        response.on('end', () => resolve(JSON.parse(Buffer.concat(chunks).toString()) as T));
        response.on('error', unreachable);
      },
    );
    call.on('error', unreachable);
    call.end(payload);
  });
}

function printRequests(lines: string[]): void {
  for (const line of lines) console.error(`[remote-cache] ${line}`);
}

async function start(argv: string[]): Promise<void> {
  // The daemon takes the number of entries per key after the directory.
  const [option, entriesPerKey] = argv;
  if (
    argv.length > 0 &&
    (argv.length !== 2 || option !== '--entries-per-key' || !/^[1-9]\d*$/.test(entriesPerKey!))
  ) {
    fail(usage);
  }
  const options = argv.length > 0 ? [entriesPerKey!] : [];
  if (existsSync(serverFile(directory))) fail('a backend is already running here.');
  mkdirSync(directory, { recursive: true });
  const log = openSync(logFile(directory), 'a');
  // The backend outlives this step, so it gets its own session and none of the
  // terminal's file descriptors. The terminal then closes when this step
  // exits, and Ctrl-C in later steps doesn't reach the backend.
  const daemon = spawn(
    process.execPath,
    [
      ...process.execArgv,
      fileURLToPath(new URL('daemon.ts', import.meta.url)),
      directory,
      ...options,
    ],
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

function parseFaultSpec(spec: string | undefined): FaultRule {
  if (spec === undefined) fail(usage);
  const sep = spec.indexOf('=');
  if (sep === -1) fail(`invalid fault spec '${spec}', expected <route>=<kind>`);
  const route = spec.slice(0, sep);
  const kind = spec.slice(sep + 1);
  if (route !== '/fetch' && route !== '/store' && route !== '/blob') {
    fail(`unknown route '${route}'`);
  }
  if (kind !== '503' && kind !== '429' && kind !== 'drop' && kind !== 'truncate') {
    fail(`unknown fault kind '${kind}'`);
  }
  if (kind === 'truncate' && route !== '/blob') {
    fail('truncate fault is only valid for /blob');
  }
  return { route: route as FaultRoute, kind: kind as FaultKind };
}

function parsePositiveInt(value: string | undefined): number {
  if (value === undefined || !/^[1-9]\d*$/.test(value)) fail(usage);
  return Number(value);
}

async function run(argv: string[]): Promise<void> {
  let githubActions = false;
  const faults: FaultRule[] = [];
  const alwaysFail: FaultRule[] = [];
  let holdStores = 0;
  let releaseAfterFetches = 0;
  let summary = false;
  let index = 0;
  while (index < argv.length) {
    const arg = argv[index]!;
    if (arg === '--github-actions') {
      githubActions = true;
      index += 1;
    } else if (arg === '--fault') {
      faults.push(parseFaultSpec(argv[index + 1]));
      index += 2;
    } else if (arg === '--always-fail') {
      alwaysFail.push(parseFaultSpec(argv[index + 1]));
      index += 2;
    } else if (arg === '--hold-stores') {
      holdStores = parsePositiveInt(argv[index + 1]);
      index += 2;
    } else if (arg === '--release-after-fetches') {
      releaseAfterFetches = parsePositiveInt(argv[index + 1]);
      index += 2;
    } else if (arg === '--summary') {
      summary = true;
      index += 1;
    } else {
      break;
    }
  }
  const [command, ...args] = argv.slice(index);
  if (command === undefined) fail(usage);
  if (releaseAfterFetches > 0 && holdStores === 0) {
    fail('--release-after-fetches requires --hold-stores');
  }
  const info = readInfo();
  const hasRunConfig = faults.length > 0 || alwaysFail.length > 0 || holdStores > 0 || summary;
  if (hasRunConfig) {
    const config: RunConfig = { faults, alwaysFail, holdStores, releaseAfterFetches, summary };
    await control(info, '/configure', 'unknown request', config);
  }
  const env: NodeJS.ProcessEnv = { ...process.env, VP_REMOTE_CACHE_URL: info.url };
  if (githubActions) {
    // As in a GitHub Actions job with `id-token: write`.
    env['ACTIONS_ID_TOKEN_REQUEST_URL'] = info.tokenRequestUrl;
    env['ACTIONS_ID_TOKEN_REQUEST_TOKEN'] ??= defaultRequestToken;
  }
  // Ctrl-C is left to the command.
  process.on('SIGINT', () => {});
  // cross-spawn runs pnpm's `.cmd` shims on Windows.
  const child = crossSpawn(command, args, { stdio: 'inherit', env });
  const [code] = (await once(child, 'exit')) as [number | null];
  printRequests(await control<string[]>(info, '/take'));
  process.exitCode = code;
}

/** Change what the backend stores for blob `number`, numbered as in the request lines. */
async function changeBlob(change: string, [number, ...rest]: string[]): Promise<void> {
  if (number === undefined || !/^[1-9]\d*$/.test(number) || rest.length > 0) fail(usage);
  await control(readInfo(), `/${change}/${number}`, `no blob ${number} has been stored.`);
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
if (subcommand === 'start') await start(args);
else if (subcommand === 'run') await run(args);
else if (subcommand === 'expire-entry' && args[0] === '--before-download') {
  await changeBlob('expire-before-download', args.slice(1));
} else if (subcommand === 'corrupt-blob' || subcommand === 'expire-entry') {
  await changeBlob(subcommand, args);
} else if (subcommand === 'stop') await stop();
else fail(usage);
