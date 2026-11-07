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
import { basePath, serverFile, type ServerInfo } from './state.ts';

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
let lastUse = Date.now();
let stopping = false;

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
  const id = /^\/blob\/(.+)$/.exec(route)?.[1];
  if (id !== undefined && expireBeforeDownload.delete(id)) {
    // The client sent this request after the fetch's response, so the entry
    // expires between the two.
    backend.expireEntry(id).then(
      () => pass(request, response, route, label),
      (error: unknown) => {
        anomalies.push(`${label} failed to expire its entry: ${String(error)}`);
        response.writeHead(502).end();
      },
    );
  } else {
    pass(request, response, route, label);
  }
});

/** Forward a request to the backend and its response back, recording the response's line. */
function pass(
  request: IncomingMessage,
  response: ServerResponse,
  route: string,
  label: string,
): void {
  let clientGone = false;
  let backendReply: IncomingMessage | undefined;
  const upstream = forward(
    `${backend.origin}${request.url}`,
    { method: request.method!, headers: endToEnd(request.headers) },
    (reply) => {
      backendReply = reply;
      const status = reply.statusCode!;
      if (status >= 500) anomalies.push(`${label} got ${status} from the backend`);
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
    // The Worker rejects an upload by its headers, answering before it reads
    // the body, and then closes the connection, so sending the rest fails.
    if (clientGone || backendReply?.complete) return;
    anomalies.push(`${label} failed: ${error.message}`);
    if (response.headersSent) response.destroy();
    else response.writeHead(502).end();
  });
  // The client can disconnect before the response ends, e.g. after Ctrl-C.
  response.on('close', () => {
    if (response.writableFinished) return;
    clientGone = true;
    upstream.destroy();
  });
  request.pipe(upstream);
}

const control = createServer((request, response) => {
  lastUse = Date.now();
  request.resume();
  const reply = (value: unknown) => {
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end(JSON.stringify(value));
  };
  const url = new URL(request.url ?? '/', 'http://localhost');
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
    reply(requests.splice(0));
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
