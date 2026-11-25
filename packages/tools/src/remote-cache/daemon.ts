import { decode } from 'cbor2/decoder';
import { once } from 'node:events';
import { existsSync, rmSync, writeFileSync } from 'node:fs';
import {
  createServer,
  request as forward,
  type IncomingHttpHeaders,
  type IncomingMessage,
  type Server,
  type ServerResponse,
} from 'node:http';
import type { AddressInfo } from 'node:net';
import { startBackend } from './backend.ts';
import { createSigningKey, repository } from './github.ts';
import {
  basePath,
  serverFile,
  type FaultKind,
  type FaultRoute,
  type RunConfig,
  type ServerInfo,
} from './state.ts';

// The process that `remote-cache-server start` leaves running for one e2e case.
// It serves the backend at the endpoint through a tap that forwards requests
// and responses unchanged and records a line for each response. Blob IDs in
// the lines become numbers in upload order, so they're the same on every run.
// A control server hands the lines to `remote-cache-server run` and `stop`,
// changes blobs and entries for `remote-cache-server corrupt-blob` and
// `expire-entry`, and stands in for GitHub Actions' token service.

const directory = process.argv[2]!;
/** The number of entries the namespace keeps per key, if not the Worker's default. */
const entriesPerKey = process.argv[3] === undefined ? undefined : Number(process.argv[3]);
/** Stop after this long without requests, e.g. when a case timed out before its stop step. */
const idleTimeout = 10 * 60 * 1000;
/** Headers about a single connection. Node sets them for each hop itself. */
const hopByHop = new Set([
  'connection',
  'keep-alive',
  'proxy-connection',
  'transfer-encoding',
  'upgrade',
]);

const signingKey = createSigningKey();
/** Request lines that no `run` or `stop` has taken yet. */
const requests: string[] = [];
/** Problems on the backend's side, which make `stop` fail. */
const anomalies: string[] = [];
/** The ID of each stored blob, in upload order. Blob number `n` is at `n - 1`. */
const blobs: string[] = [];
/** The blobs whose entries expire when the blob is next requested. */
const expireBeforeDownload = new Set<string>();
/** Per-run FIFO fault queues configured by `POST /configure`. */
const faults = new Map<FaultRoute, FaultKind[]>();
/** Per-run permanent faults configured by `POST /configure`. */
const alwaysFail = new Map<FaultRoute, FaultKind>();

interface StoreGate {
  holdTarget: number;
  releaseAfterFetches: number;
  inFlight: number;
  maxInFlight: number;
  fetches: number;
  released: boolean;
  waiters: Array<() => void>;
}

let storeGate: StoreGate | undefined;
let summaryMode = false;
let lastUse = Date.now();
let stopping = false;

function matchFaultRoute(route: string): FaultRoute | undefined {
  if (route === '/fetch' || route === '/store') return route;
  if (route.startsWith('/blob/')) return '/blob';
  return undefined;
}

function nextFault(route: string): FaultKind | undefined {
  const faultRoute = matchFaultRoute(route);
  if (faultRoute === undefined) return undefined;
  return alwaysFail.get(faultRoute) ?? faults.get(faultRoute)?.shift();
}

function releaseStoreGateIfReady(): void {
  if (
    storeGate !== undefined &&
    !storeGate.released &&
    storeGate.inFlight >= storeGate.holdTarget &&
    storeGate.fetches >= storeGate.releaseAfterFetches
  ) {
    storeGate.released = true;
    for (const wake of storeGate.waiters.splice(0)) wake();
  }
}

function resetRunConfig(): void {
  faults.clear();
  alwaysFail.clear();
  if (storeGate !== undefined) {
    storeGate.released = true;
    for (const wake of storeGate.waiters.splice(0)) wake();
    storeGate = undefined;
  }
  summaryMode = false;
}

function summarizeRequests(taken: string[], maxConcurrentStores: number | undefined): string[] {
  const counts = new Map<string, number>();
  for (const line of taken) counts.set(line, (counts.get(line) ?? 0) + 1);
  const lines = [...counts.entries()]
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([line, count]) => `${line} x${count}`);
  if (maxConcurrentStores !== undefined) {
    lines.push(`max_concurrent_stores=${maxConcurrentStores}`);
  }
  return lines;
}

function endToEnd(headers: IncomingHttpHeaders): IncomingHttpHeaders {
  return Object.fromEntries(Object.entries(headers).filter(([name]) => !hopByHop.has(name)));
}

/** The fields of a CBOR map. */
function cborFields(body: Buffer): Record<string, unknown> {
  try {
    const value: unknown = decode(body);
    if (typeof value === 'object' && value !== null) return value as Record<string, unknown>;
  } catch {
    // Not CBOR. vp reports the malformed response itself.
  }
  return {};
}

/**
 * The blob numbers of an exact fetch's entries, newest first, e.g. `[3, 1]`.
 * An entry without a blob shows `-`.
 */
function entryBlobs(entries: unknown): string | undefined {
  if (!Array.isArray(entries)) return undefined;
  const numbers = entries.map((entry: unknown) => {
    const id =
      typeof entry === 'object' && entry !== null
        ? (entry as Record<string, unknown>)['blob_id']
        : undefined;
    if (id === null) return '-';
    const index = typeof id === 'string' ? blobs.indexOf(id) : -1;
    return index === -1 ? '?' : String(index + 1);
  });
  return `[${numbers.join(', ')}]`;
}

/** `route` with a stored blob's ID replaced by its number. */
function numbered(route: string): string {
  const id = /^\/blob\/(.+)$/.exec(route)?.[1];
  const index = id === undefined ? -1 : blobs.indexOf(id);
  return index === -1 ? route : `/blob/${index + 1}`;
}

const tap = createServer((request, response) => {
  lastUse = Date.now();
  const path = new URL(request.url ?? '/', 'http://localhost').pathname;
  const route = path.startsWith(basePath) ? path.slice(basePath.length) : path;
  const label = `${request.method} ${numbered(route)}`;
  const fault = nextFault(route);
  if (fault !== undefined && fault !== 'truncate') {
    // Drain the full request body before replying or closing so the client
    // never sees a write-side reset before reading the injected status.
    request.resume();
    request.on('end', () => {
      if (fault === '503') {
        requests.push(`${label} 503`);
        response.writeHead(503, {
          'retry-after': '60',
          connection: 'close',
          'content-length': '4',
        });
        response.end('busy');
      } else if (fault === '429') {
        requests.push(`${label} 429`);
        response.writeHead(429, {
          'retry-after': '5',
          connection: 'close',
          'content-length': '12',
        });
        response.end('rate limited');
      } else {
        requests.push(`${label} drop`);
        response.destroy();
      }
    });
    return;
  }
  if (route === '/fetch' && storeGate !== undefined) {
    storeGate.fetches += 1;
    releaseStoreGateIfReady();
  }
  const id = /^\/blob\/(.+)$/.exec(route)?.[1];
  if (id !== undefined && expireBeforeDownload.delete(id)) {
    // The client sent this request after the fetch's response, so the entry
    // expires between the two.
    backend.expireEntry(id).then(
      () => pass(request, response, route, label, fault === 'truncate'),
      (error: unknown) => {
        anomalies.push(`${label} failed to expire its entry: ${String(error)}`);
        response.writeHead(502).end();
      },
    );
  } else {
    pass(request, response, route, label, fault === 'truncate');
  }
});

/** Forward a request to the backend and its response back, recording the response's line. */
function pass(
  request: IncomingMessage,
  response: ServerResponse,
  route: string,
  label: string,
  truncateBlob: boolean,
): void {
  let clientGone = false;
  let backendReply: IncomingMessage | undefined;
  let upstream: ReturnType<typeof forward> | undefined;
  const activeGate = route === '/store' ? storeGate : undefined;
  let countedInFlight = false;
  const releaseInFlight = () => {
    if (countedInFlight && activeGate !== undefined) {
      countedInFlight = false;
      activeGate.inFlight -= 1;
    }
  };

  // The client can disconnect before the response ends, e.g. after Ctrl-C.
  response.on('close', () => {
    if (response.writableFinished) return;
    clientGone = true;
    releaseInFlight();
    upstream?.destroy();
  });

  // Buffer the body once it has all arrived, so it goes out with the headers
  // before the Worker can answer. Sending part of it after a rejection fails
  // if the connection was reset, and that can lose the answer before it's
  // read.
  const body: Buffer[] = [];
  request.on('data', (chunk: Buffer) => body.push(chunk));
  request.on('end', () => {
    void forwardBuffered(Buffer.concat(body));
  });

  async function forwardBuffered(payload: Buffer): Promise<void> {
    if (clientGone) return;
    if (activeGate !== undefined) {
      activeGate.inFlight += 1;
      activeGate.maxInFlight = Math.max(activeGate.maxInFlight, activeGate.inFlight);
      countedInFlight = true;
      releaseStoreGateIfReady();
      if (!activeGate.released) {
        await new Promise<void>((resolve) => {
          activeGate.waiters.push(resolve);
        });
      }
      if (clientGone) {
        releaseInFlight();
        return;
      }
    }
    upstream = forward(
      `${backend.origin}${request.url}`,
      { method: request.method!, headers: endToEnd(request.headers) },
      (reply) => {
        backendReply = reply;
        const status = reply.statusCode!;
        if (status >= 500) anomalies.push(`${label} got ${status} from the backend`);
        if (truncateBlob && status === 200) {
          const blobChunks: Buffer[] = [];
          reply.on('data', (chunk: Buffer) => blobChunks.push(chunk));
          reply.on('end', () => {
            const full = Buffer.concat(blobChunks);
            const cut = Math.min(Math.floor(full.length / 2), Math.max(0, full.length - 1));
            requests.push(`${label} truncate`);
            response.writeHead(200, {
              'content-length': String(full.length),
              connection: 'close',
            });
            response.write(full.subarray(0, cut), () => {
              response.destroy();
            });
          });
          return;
        }
        // Decrement in-flight count strictly before writing headers so the
        // client cannot release its upload slot and overlap the next store.
        releaseInFlight();
        response.writeHead(status, endToEnd(reply.headers));
        // Fetch and store responses are CBOR maps with the kind and entries, or
        // the blob ID.
        const chunks: Buffer[] = [];
        if ((route === '/fetch' || route === '/store') && status === 200) {
          reply.on('data', (chunk: Buffer) => chunks.push(chunk));
        }
        reply.pipe(response);
        response.on('finish', () => {
          const fields = chunks.length > 0 ? cborFields(Buffer.concat(chunks)) : {};
          const blobId = fields['blob_id'];
          if (route === '/store' && typeof blobId === 'string') blobs.push(blobId);
          const kind = route === '/fetch' ? fields['kind'] : undefined;
          const entries = route === '/fetch' ? entryBlobs(fields['entries']) : undefined;
          requests.push(
            [label, status, kind, entries].filter((part) => part !== undefined).join(' '),
          );
        });
      },
    );
    upstream.on('error', (error) => {
      releaseInFlight();
      // The Worker rejects an upload by its headers, answering before it reads
      // the body, and then closes the connection, which can reset it after the
      // answer.
      if (clientGone || backendReply?.complete) return;
      anomalies.push(`${label} failed: ${error.message}`);
      if (response.headersSent) response.destroy();
      else response.writeHead(502).end();
    });
    upstream.end(payload);
  }
}

const control = createServer((request, response) => {
  lastUse = Date.now();
  const reply = (value: unknown) => {
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end(JSON.stringify(value));
  };
  const url = new URL(request.url ?? '/', 'http://localhost');
  if (request.method === 'POST' && request.url === '/configure') {
    const chunks: Buffer[] = [];
    request.on('data', (chunk: Buffer) => chunks.push(chunk));
    request.on('end', () => {
      const config = JSON.parse(Buffer.concat(chunks).toString()) as RunConfig;
      resetRunConfig();
      for (const rule of config.faults) {
        const queue = faults.get(rule.route) ?? [];
        queue.push(rule.kind);
        faults.set(rule.route, queue);
      }
      for (const rule of config.alwaysFail) {
        alwaysFail.set(rule.route, rule.kind);
      }
      if (config.holdStores > 0) {
        storeGate = {
          holdTarget: config.holdStores,
          releaseAfterFetches: config.releaseAfterFetches,
          inFlight: 0,
          maxInFlight: 0,
          fetches: 0,
          released: false,
          waiters: [],
        };
      }
      summaryMode = config.summary;
      reply(null);
    });
    return;
  }
  request.resume();
  if (request.method === 'GET' && url.pathname === '/token') {
    // The request token stands for the workflow run, as in GitHub Actions.
    const requestToken = /^Bearer (.+)$/.exec(request.headers.authorization ?? '')?.[1];
    const audience = url.searchParams.get('audience');
    const value =
      requestToken === undefined || audience === null
        ? undefined
        : signingKey.issue(requestToken, audience);
    if (value === undefined) response.writeHead(401).end();
    else reply({ count: 1, value });
    return;
  }
  const [, change, number] =
    /^\/(corrupt-blob|expire-entry|expire-before-download)\/(\d+)$/.exec(request.url ?? '') ?? [];
  if (request.method === 'POST' && request.url === '/take') {
    const taken = requests.splice(0);
    const lines = summaryMode ? summarizeRequests(taken, storeGate?.maxInFlight) : taken;
    resetRunConfig();
    reply(lines);
  } else if (request.method === 'POST' && number !== undefined) {
    const id = blobs[Number(number) - 1];
    if (id === undefined) {
      response.writeHead(404).end();
      return;
    }
    if (change === 'expire-before-download') {
      expireBeforeDownload.add(id);
      reply(null);
      return;
    }
    const changed =
      change === 'corrupt-blob'
        ? backend.writeBlob(id, Buffer.from('corrupt'))
        : backend.expireEntry(id);
    changed.then(
      () => reply(null),
      (error: unknown) => {
        console.error(error);
        response.writeHead(500).end();
      },
    );
  } else if (request.method === 'POST' && request.url === '/stop') {
    response.on('finish', () => void stop());
    reply({ requests: requests.splice(0), anomalies });
  } else {
    response.writeHead(404).end();
  }
});

async function listen(server: Server): Promise<number> {
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  return (server.address() as AddressInfo).port;
}

async function stop(): Promise<void> {
  if (stopping) return;
  stopping = true;
  rmSync(serverFile(directory), { force: true });
  for (const server of [tap, control]) {
    server.close();
    server.closeAllConnections();
  }
  await backend.close();
  process.exit(0);
}

const controlOrigin = `http://127.0.0.1:${await listen(control)}`;
const info: ServerInfo = {
  url: `http://127.0.0.1:${await listen(tap)}${basePath}`,
  control: controlOrigin,
  tokenRequestUrl: `${controlOrigin}/token?api-version=2.0`,
};
// Upload tokens carry the endpoint as their audience, so the backend learns it
// before the first request.
const backend = await startBackend({
  basePath,
  directory,
  endpoint: info.url,
  entriesPerKey,
  key: signingKey,
  registered: repository,
});
writeFileSync(serverFile(directory), `${JSON.stringify(info, null, 2)}\n`);
process.on('SIGTERM', () => void stop());
setInterval(() => {
  // The e2e harness removes case directories when the test run ends.
  if (!existsSync(directory) || Date.now() - lastUse > idleTimeout) void stop();
}, 1000);
process.send?.('ready');
process.disconnect?.();
