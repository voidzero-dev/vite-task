import { Busboy } from '@fastify/busboy';
import { decode } from 'cbor2/decoder';
import { encode } from 'cbor2/encoder';
import { randomUUID, verify } from 'node:crypto';
import { once } from 'node:events';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import type { AddressInfo } from 'node:net';
import { join } from 'node:path';
import { issuer, type repository, type TrustedKey } from './github.ts';

interface Entry {
  value: string;
  blob_id: string | null;
}

/** The contents of `state.json`. Keys and values are hex-encoded. */
interface State {
  entries: Record<string, Entry>;
  associations: Record<string, string>;
}

class RequestError extends Error {
  status: number;

  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

function mediaType(value: string | undefined): string {
  return value?.split(';')[0]?.trim().toLowerCase() ?? '';
}

function toHex(bytes: Uint8Array): string {
  return Buffer.from(bytes).toString('hex');
}

function fromHex(hex: string): Uint8Array {
  // Encode Uint8Array, not a Node Buffer, whose toJSON method would otherwise
  // turn it into a map.
  return new Uint8Array(Buffer.from(hex, 'hex'));
}

function byteFields(body: Uint8Array, names: string[]): Map<string, Uint8Array> {
  let value: unknown;
  try {
    value = decode(body, { preferMap: true, rejectDuplicateKeys: true });
  } catch {
    throw new RequestError(400, 'Invalid CBOR');
  }
  if (!(value instanceof Map) || names.some((name) => !(value.get(name) instanceof Uint8Array))) {
    throw new RequestError(400, `Expected byte strings: ${names.join(', ')}`);
  }
  return value;
}

function readBody(request: IncomingMessage): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    request.on('data', (chunk: Buffer) => chunks.push(chunk));
    request.on('end', () => resolve(Buffer.concat(chunks)));
    request.on('error', reject);
    request.on('aborted', () => reject(new RequestError(400, 'Incomplete request')));
  });
}

async function readParts(body: Buffer, contentType: string): Promise<Map<string, Buffer>> {
  return new Promise((resolve, reject) => {
    const parts = new Map<string, Buffer>();
    const names = new Set<string>();
    // Both parts are binary, including metadata without a filename.
    const parser = new Busboy({
      headers: { 'content-type': contentType },
      isPartAFile: () => true,
    });
    parser.on('file', (name, stream, _filename, _encoding, type) => {
      const expected = name === 'metadata' ? 'application/cbor' : 'application/octet-stream';
      if (!['metadata', 'blob'].includes(name) || names.has(name) || type !== expected) {
        reject(new Error('Invalid multipart part'));
      }
      names.add(name);
      const chunks: Buffer[] = [];
      stream.on('data', (chunk: Buffer) => chunks.push(chunk));
      stream.on('end', () => parts.set(name, Buffer.concat(chunks)));
      stream.on('error', reject);
    });
    parser.on('error', reject);
    parser.on('finish', () => resolve(parts));
    parser.end(body);
  });
}

function decodePart(part: string): Record<string, unknown> | undefined {
  try {
    const value: unknown = JSON.parse(Buffer.from(part, 'base64url').toString());
    return typeof value === 'object' && value !== null
      ? (value as Record<string, unknown>)
      : undefined;
  } catch {
    return undefined;
  }
}

/**
 * The claims of `token`, if it's an RS256 JSON Web Token that `key` signed for
 * GitHub's issuer and it's valid now, with the time bounds the service uses.
 */
function verifiedClaims(token: string, key: TrustedKey): Record<string, unknown> | undefined {
  const [header, payload, signature] = token.split('.') as [string, string, string];
  const protectedHeader = decodePart(header);
  if (protectedHeader?.['alg'] !== 'RS256' || protectedHeader['kid'] !== key.kid) return undefined;
  const signed = Buffer.from(`${header}.${payload}`);
  if (!verify('sha256', signed, key.publicKey, Buffer.from(signature, 'base64url')))
    return undefined;
  const claims = decodePart(payload);
  const [exp, nbf, iat] = [claims?.['exp'], claims?.['nbf'], claims?.['iat']];
  const now = Date.now() / 1000;
  if (
    claims?.['iss'] !== issuer ||
    !Number.isSafeInteger(exp) ||
    !Number.isSafeInteger(nbf) ||
    !Number.isSafeInteger(iat)
  ) {
    return undefined;
  }
  const [expires, notBefore, issued] = [exp as number, nbf as number, iat as number];
  const valid =
    expires > now &&
    notBefore <= now + 30 &&
    issued <= now + 30 &&
    issued >= now - 930 &&
    notBefore <= expires &&
    issued < expires &&
    expires - issued <= 900;
  return valid ? claims : undefined;
}

function cbor(response: ServerResponse, value: unknown): void {
  response.writeHead(200, { 'content-type': 'application/cbor' });
  response.end(encode(value));
}

/** A running backend. */
export interface Backend {
  /** Where the backend listens, e.g. `http://127.0.0.1:1234`, without a path. */
  origin: string;
  /** Replace the contents of the blob with ID `id`. */
  writeBlob(id: string, contents: Uint8Array): Promise<void>;
  close(): Promise<void>;
}

/**
 * Start a test backend on a free loopback port that keeps its state in
 * `directory`: entries and associations in `state.json`, and each blob in
 * `blobs/` under its ID, a random UUID. Keys, values, and blobs remain opaque
 * bytes. A fetch that matches neither key gets a plain-text 404.
 *
 * Like the public cache service, the backend only accepts a store with a
 * GitHub Actions token whose audience is `endpoint`, for a push to the main
 * branch of `registered`. It trusts tokens signed with `key` in place of
 * GitHub's. It answers other stores as the service does: 401 for a missing or
 * invalid token, and 403 for a token that the write policy doesn't allow.
 */
export async function startBackend({
  basePath,
  directory,
  endpoint,
  key,
  registered,
}: {
  basePath: string;
  directory: string;
  endpoint: string;
  key: TrustedKey;
  registered: typeof repository;
}): Promise<Backend> {
  const stateFile = join(directory, 'state.json');
  const blobDirectory = join(directory, 'blobs');
  const state: State = existsSync(stateFile)
    ? JSON.parse(readFileSync(stateFile, 'utf8'))
    : { entries: {}, associations: {} };
  const entries = new Map(Object.entries(state.entries));

  function authorize(authorization: string | undefined): void {
    const token = /^Bearer ([\w-]+\.[\w-]+\.[\w-]+)$/i.exec(authorization ?? '')?.[1];
    const claims = token === undefined ? undefined : verifiedClaims(token, key);
    if (claims === undefined) throw new RequestError(401, 'Invalid credentials');
    if (
      claims['aud'] !== endpoint ||
      claims['repository_id'] !== registered.id ||
      claims['repository_owner_id'] !== registered.ownerId ||
      claims['repository_visibility'] !== 'public' ||
      claims['ref'] !== registered.branch ||
      claims['ref_type'] !== 'branch' ||
      claims['event_name'] !== 'push'
    ) {
      throw new RequestError(403, 'Write not permitted');
    }
  }
  const associations = new Map(Object.entries(state.associations));

  async function handle(
    request: IncomingMessage,
    response: ServerResponse,
    path: string,
  ): Promise<void> {
    if (request.method === 'GET' && path.startsWith(`${basePath}/blob/`)) {
      const file = join(blobDirectory, path.slice(`${basePath}/blob/`.length));
      if (!existsSync(file)) throw new RequestError(404, 'Blob not found');
      response.writeHead(200, { 'content-type': 'application/octet-stream' });
      response.end(readFileSync(file));
      return;
    }
    if (request.method !== 'POST' || ![`${basePath}/fetch`, `${basePath}/store`].includes(path)) {
      throw new RequestError(404, 'Route not found');
    }

    if (path === `${basePath}/store`) authorize(request.headers.authorization);
    const contentType = request.headers['content-type'] ?? '';
    const body = await readBody(request);
    if (path === `${basePath}/fetch`) {
      if (mediaType(contentType) !== 'application/cbor') {
        throw new RequestError(400, 'Expected application/cbor');
      }
      const fields = byteFields(body, ['key', 'secondary_key']);
      const exact = entries.get(toHex(fields.get('key')!));
      const associatedKey = associations.get(toHex(fields.get('secondary_key')!));
      const fallback = associatedKey === undefined ? undefined : entries.get(associatedKey);
      if (exact) {
        cbor(response, { kind: 'exact', value: fromHex(exact.value), blob_id: exact.blob_id });
        return;
      }
      if (fallback) {
        cbor(response, {
          kind: 'fallback',
          key: fromHex(associatedKey!),
          value: fromHex(fallback.value),
          blob_id: fallback.blob_id,
        });
        return;
      }
      throw new RequestError(404, 'Not found');
    }

    if (mediaType(contentType) !== 'multipart/form-data') {
      throw new RequestError(400, 'Expected multipart/form-data');
    }
    let parts: Map<string, Buffer>;
    try {
      parts = await readParts(body, contentType);
    } catch {
      throw new RequestError(400, 'Invalid multipart body');
    }
    const metadata = parts.get('metadata');
    if (metadata === undefined) throw new RequestError(400, 'Missing metadata');
    const fields = byteFields(metadata, ['key', 'secondary_key', 'value']);
    const key = toHex(fields.get('key')!);
    const blob = parts.get('blob');
    mkdirSync(blobDirectory, { recursive: true });
    const blobId = blob === undefined ? null : randomUUID();
    if (blobId !== null) writeFileSync(join(blobDirectory, blobId), blob!);
    entries.set(key, { value: toHex(fields.get('value')!), blob_id: blobId });
    associations.set(toHex(fields.get('secondary_key')!), key);
    const saved: State = {
      entries: Object.fromEntries(entries),
      associations: Object.fromEntries(associations),
    };
    writeFileSync(stateFile, `${JSON.stringify(saved, null, 2)}\n`);
    cbor(response, { blob_id: blobId });
  }

  const server = createServer((request, response) => {
    const path = new URL(request.url ?? '/', 'http://localhost').pathname;
    handle(request, response, path).catch((error: unknown) => {
      const known = error instanceof RequestError;
      if (!known) console.error(error);
      response.writeHead(known ? error.status : 500, {
        'content-type': 'text/plain; charset=utf-8',
      });
      response.end(known ? error.message : 'Internal server error');
      request.resume();
    });
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const { port } = server.address() as AddressInfo;
  return {
    origin: `http://127.0.0.1:${port}`,
    writeBlob: async (id, contents) => writeFileSync(join(blobDirectory, id), contents),
    close: () =>
      new Promise((resolve) => {
        server.close(() => resolve());
        server.closeAllConnections();
      }),
  };
}
