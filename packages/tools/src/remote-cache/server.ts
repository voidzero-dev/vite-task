import { Busboy } from '@fastify/busboy';
import { decode } from 'cbor2/decoder';
import { encode } from 'cbor2/encoder';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { join } from 'node:path';

interface Entry {
  value: string;
  blob_id: string | null;
}

/** The contents of `state.json`. Keys and values are hex-encoded. */
interface State {
  next_blob_id: number;
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

function cbor(response: ServerResponse, value: unknown): void {
  response.writeHead(200, { 'content-type': 'application/cbor' });
  response.end(encode(value));
}

function loadState(file: string): State {
  try {
    return JSON.parse(readFileSync(file, 'utf8')) as State;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
    return { next_blob_id: 1, entries: {}, associations: {} };
  }
}

async function readBlob(file: string): Promise<Buffer | undefined> {
  try {
    return await readFile(file);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return undefined;
    throw error;
  }
}

/**
 * A test backend that keeps its state in `directory`: entries and associations
 * in `state.json`, and each blob in `blobs/` under its ID. Keys, values, and
 * blobs remain opaque bytes.
 */
export function createCacheServer({
  basePath,
  directory,
}: {
  basePath: string;
  directory: string;
}) {
  const stateFile = join(directory, 'state.json');
  const blobDirectory = join(directory, 'blobs');
  const state = loadState(stateFile);
  const entries = new Map(Object.entries(state.entries));
  const associations = new Map(Object.entries(state.associations));
  let nextBlobId = state.next_blob_id;

  async function handle(request: IncomingMessage, response: ServerResponse): Promise<void> {
    const path = new URL(request.url ?? '/', 'http://localhost').pathname;
    if (request.method === 'GET' && path.startsWith(`${basePath}/blob/`)) {
      const blobId = path.slice(`${basePath}/blob/`.length);
      // Blob IDs are sequential numbers, so other IDs cannot name a blob file.
      const blob = /^\d+$/.test(blobId) ? await readBlob(join(blobDirectory, blobId)) : undefined;
      if (blob === undefined) throw new RequestError(404, 'Blob not found');
      response.writeHead(200, { 'content-type': 'application/octet-stream' });
      response.end(blob);
      return;
    }
    if (request.method !== 'POST' || ![`${basePath}/fetch`, `${basePath}/store`].includes(path)) {
      throw new RequestError(404, 'Route not found');
    }

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
      } else if (fallback) {
        cbor(response, {
          kind: 'fallback',
          key: fromHex(associatedKey!),
          value: fromHex(fallback.value),
          blob_id: fallback.blob_id,
        });
      } else {
        cbor(response, { kind: 'not_found' });
      }
      return;
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

    // Publish only after the complete request is validated. The writes are
    // synchronous, so concurrent stores cannot interleave them.
    mkdirSync(blobDirectory, { recursive: true });
    const blobId = blob === undefined ? null : String(nextBlobId++);
    if (blobId !== null) writeFileSync(join(blobDirectory, blobId), blob!);
    entries.set(key, { value: toHex(fields.get('value')!), blob_id: blobId });
    associations.set(toHex(fields.get('secondary_key')!), key);
    const saved: State = {
      next_blob_id: nextBlobId,
      entries: Object.fromEntries(entries),
      associations: Object.fromEntries(associations),
    };
    writeFileSync(stateFile, `${JSON.stringify(saved, null, 2)}\n`);
    cbor(response, { blob_id: blobId });
  }

  return createServer((request, response) => {
    void handle(request, response).catch((error: unknown) => {
      const known = error instanceof RequestError;
      if (!known) console.error(error);
      response.writeHead(known ? error.status : 500, {
        'content-type': 'text/plain; charset=utf-8',
      });
      response.end(known ? error.message : 'Internal server error');
      request.resume();
    });
  });
}
