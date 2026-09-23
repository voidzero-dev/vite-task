import { Busboy } from '@fastify/busboy';
import { decode } from 'cbor2/decoder';
import { encode } from 'cbor2/encoder';
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';

interface Entry {
  value: Uint8Array;
  blob_id: string | null;
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

function byteFields(body: Uint8Array, names: string[]): Map<string, Uint8Array> {
  let value: unknown;
  try {
    // Decode from Uint8Array so byte strings stay Uint8Array, not Node Buffers
    // (whose toJSON method would otherwise turn them into maps when encoded).
    value = decode(new Uint8Array(body), { preferMap: true, rejectDuplicateKeys: true });
  } catch {
    throw new RequestError(400, 'Invalid CBOR');
  }
  if (!(value instanceof Map) || names.some((name) => !(value.get(name) instanceof Uint8Array))) {
    throw new RequestError(400, `Expected byte strings: ${names.join(', ')}`);
  }
  return value;
}

function readBody(request: IncomingMessage, limit: number): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    let size = 0;
    request.on('data', (chunk: Buffer) => {
      size += chunk.length;
      if (size > limit) {
        chunks.length = 0;
        reject(new RequestError(413, 'Request too large'));
      } else {
        chunks.push(chunk);
      }
    });
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

/** A disposable backend. Keys, values, and blobs remain opaque bytes. */
export function createCacheServer({ maxRequestBytes = 64 * 1024 * 1024, basePath = '' } = {}) {
  const entries = new Map<string, Entry>();
  const associations = new Map<string, string>();
  const blobs = new Map<string, Buffer>();
  let nextBlobId = 1;

  async function handle(request: IncomingMessage, response: ServerResponse): Promise<void> {
    const path = new URL(request.url ?? '/', 'http://localhost').pathname;
    if (request.method === 'GET' && path.startsWith(`${basePath}/blob/`)) {
      const blob = blobs.get(path.slice(`${basePath}/blob/`.length));
      if (blob === undefined) throw new RequestError(404, 'Blob not found');
      response.writeHead(200, { 'content-type': 'application/octet-stream' });
      response.end(blob);
      return;
    }
    if (request.method !== 'POST' || ![`${basePath}/fetch`, `${basePath}/store`].includes(path)) {
      throw new RequestError(404, 'Route not found');
    }

    const contentType = request.headers['content-type'] ?? '';
    const body = await readBody(request, maxRequestBytes);
    if (path === `${basePath}/fetch`) {
      if (mediaType(contentType) !== 'application/cbor') {
        throw new RequestError(400, 'Expected application/cbor');
      }
      const fields = byteFields(body, ['key', 'secondary_key']);
      const key = Buffer.from(fields.get('key')!).toString('hex');
      const secondary = Buffer.from(fields.get('secondary_key')!).toString('hex');
      const entry = entries.get(key);
      if (entry) {
        cbor(response, { kind: 'exact', ...entry });
      } else {
        const associatedKey = associations.get(secondary);
        if (associatedKey === undefined || !entries.has(associatedKey)) {
          throw new RequestError(404, 'Entry not found');
        }
        cbor(response, {
          kind: 'fallback',
          key: new Uint8Array(Buffer.from(associatedKey, 'hex')),
        });
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
    const key = Buffer.from(fields.get('key')!).toString('hex');
    const secondary = Buffer.from(fields.get('secondary_key')!).toString('hex');
    const blob = parts.get('blob');
    const blobId = blob === undefined ? null : String(nextBlobId++);

    // Publish only after the complete request is validated. There is no await
    // between these writes, so concurrent stores cannot mix values and blobs.
    if (blobId !== null) blobs.set(blobId, blob!);
    entries.set(key, { value: fields.get('value')!, blob_id: blobId });
    associations.set(secondary, key);
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
