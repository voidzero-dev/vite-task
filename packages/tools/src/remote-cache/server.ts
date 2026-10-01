import { decode } from 'cbor2/decoder';
import { build } from 'esbuild';
import { Miniflare, convertV4MiniflareOptions } from 'miniflare';
import { generateKeyPairSync, sign, type KeyObject } from 'node:crypto';
import { once } from 'node:events';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { createServer, type IncomingMessage } from 'node:http';
import type { AddressInfo } from 'node:net';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const serviceDirectory = new URL(
  './',
  import.meta.resolve('@voidzero-dev/remote-cache/package.json'),
);

/** The token issuer the service trusts for uploads: GitHub Actions. */
const issuer = 'https://token.actions.githubusercontent.com';

/**
 * Requests reach the service at this origin, whatever address the proxy
 * listens on. `endpoint` is also the audience of upload tokens.
 */
const origin = 'https://cache.example';
const namespace = 'test';
const basePath = `/projects/${namespace}`;
const endpoint = `${origin}${basePath}`;

/** Upload token claims that satisfy the namespace's write policy. */
const writerClaims = {
  repository_id: '1',
  repository_owner_id: '1',
  repository_visibility: 'public',
  ref: 'refs/heads/main',
  ref_type: 'branch',
  event_name: 'push',
};

/** Request headers that describe the connection to the proxy. */
const connectionHeaders = new Set(['connection', 'content-length', 'host', 'transfer-encoding']);

type Database = Awaited<ReturnType<Miniflare['getD1Database']>>;
type Bucket = Awaited<ReturnType<Miniflare['getR2Bucket']>>;

interface ResponseFields {
  kind?: unknown;
  blob_id?: unknown;
}

function readBody(request: IncomingMessage): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    request.on('data', (chunk: Buffer) => chunks.push(chunk));
    request.on('end', () => resolve(Buffer.concat(chunks)));
    request.on('error', reject);
  });
}

/** The fields of a CBOR response, or none if it isn't a CBOR map. */
function cborFields(contentType: string | null, body: Uint8Array): ResponseFields {
  if (contentType !== 'application/cbor') return {};
  try {
    const value = decode(body);
    return typeof value === 'object' && value !== null ? value : {};
  } catch {
    return {};
  }
}

async function bundleService(): Promise<string> {
  const bundle = await build({
    entryPoints: [fileURLToPath(new URL('src/index.ts', serviceDirectory))],
    bundle: true,
    write: false,
    format: 'esm',
    platform: 'neutral',
    target: 'es2022',
    external: ['node:*', 'cloudflare:*'],
  });
  return bundle.outputFiles[0]!.text;
}

/** Apply the service's schema and register the namespace, unless done before. */
async function initializeDatabase(database: Database): Promise<void> {
  const initialized = await database
    .prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'scopes'")
    .first();
  if (initialized) return;
  const migration = readFileSync(new URL('migrations/0001_cache.sql', serviceDirectory), 'utf8');
  // Keep each trigger body in one statement.
  const statements = migration.match(
    /CREATE TRIGGER[\s\S]*?\nEND;|(?:CREATE TABLE|CREATE (?:UNIQUE )?INDEX|INSERT INTO)[\s\S]*?;/g,
  )!;
  await database.batch(statements.map((sql) => database.prepare(sql)));
  await database
    .prepare(
      `INSERT INTO scopes (scope_id, endpoint, repository, repository_id, repository_owner_id, branch)
      VALUES (?, ?, 'owner/repository', ?, ?, ?)`,
    )
    .bind(
      namespace,
      endpoint,
      writerClaims.repository_id,
      writerClaims.repository_owner_id,
      writerClaims.ref,
    )
    .run();
}

/** An upload token signed with `privateKey`, whose public key is `kid`. */
function uploadToken(privateKey: KeyObject, kid: string): string {
  const now = Math.floor(Date.now() / 1000);
  const encode = (value: object) => Buffer.from(JSON.stringify(value)).toString('base64url');
  const header = encode({ alg: 'RS256', kid, typ: 'JWT' });
  const claims = encode({
    ...writerClaims,
    iss: issuer,
    aud: endpoint,
    iat: now,
    nbf: now,
    exp: now + 300,
  });
  const signature = sign('sha256', Buffer.from(`${header}.${claims}`), privateKey);
  return `${header}.${claims}.${signature.toString('base64url')}`;
}

/**
 * Blob IDs are random, so blobs are numbered in upload order. The numbers are
 * saved in `blobs.json`, and each blob has a copy in `blobs/{number}`.
 */
class Blobs {
  readonly #file: string;
  readonly #directory: string;
  readonly #ids: string[];
  readonly #database: Database;
  readonly #bucket: Bucket;

  constructor(directory: string, database: Database, bucket: Bucket) {
    this.#file = join(directory, 'blobs.json');
    this.#directory = join(directory, 'blobs');
    this.#ids = existsSync(this.#file) ? JSON.parse(readFileSync(this.#file, 'utf8')) : [];
    this.#database = database;
    this.#bucket = bucket;
  }

  /** Number the blob `id` if it's new. */
  add(id: string): void {
    if (!this.#ids.includes(id)) this.#ids.push(id);
  }

  /** The number of the blob `id`, or `undefined` if it has none. */
  find(id: string): number | undefined {
    const index = this.#ids.indexOf(id);
    return index === -1 ? undefined : index + 1;
  }

  /** Store the contents of each copy that differs from its blob. */
  async replaceEdited(): Promise<void> {
    for (const [id, file, object] of await this.#entries()) {
      if (!existsSync(file)) continue;
      const bytes = readFileSync(file);
      const stored = await this.#bucket.get(object);
      if (stored && Buffer.from(await stored.arrayBuffer()).equals(bytes)) continue;
      await this.#bucket.put(object, bytes);
      // The service checks the size of each blob it serves.
      await this.#database
        .prepare('UPDATE generations SET blob_size = ? WHERE blob_id = ?')
        .bind(bytes.length, id)
        .run();
    }
  }

  /** Save the numbers and a copy of each blob. */
  async save(): Promise<void> {
    mkdirSync(this.#directory, { recursive: true });
    writeFileSync(this.#file, `${JSON.stringify(this.#ids, null, 2)}\n`);
    for (const [, file, object] of await this.#entries()) {
      const stored = await this.#bucket.get(object);
      if (stored) writeFileSync(file, Buffer.from(await stored.arrayBuffer()));
    }
  }

  /** The ID, copy, and storage object of each numbered blob still stored. */
  async #entries(): Promise<[string, string, string][]> {
    const entries: [string, string, string][] = [];
    for (const [index, id] of this.#ids.entries()) {
      const object = await this.#database
        .prepare('SELECT blob_object FROM generations WHERE blob_id = ?')
        .bind(id)
        .first<string>('blob_object');
      if (object !== null) entries.push([id, join(this.#directory, String(index + 1)), object]);
    }
    return entries;
  }
}

/**
 * Run the public cache service from `packages/remote-cache` in workerd and
 * serve it through a proxy on a free loopback port. The service keeps its D1
 * database and R2 bucket in `directory/state`, so consecutive servers share
 * them.
 *
 * The proxy adds a signed upload token to each store request, and the service
 * verifies it with a key served in place of GitHub's. After each response,
 * `logRequest` receives a line with the method, the route below the endpoint,
 * the status, and for successful fetches, the kind. Blob routes show the
 * blob's number instead of its ID.
 *
 * When the server closes, each blob is copied to `directory/blobs/{number}`.
 * The next server stores a copy that has changed as that blob's contents.
 */
export async function startCacheServer({
  directory,
  logRequest,
}: {
  directory: string;
  logRequest: (line: string) => void;
}): Promise<{ url: string; close: () => Promise<void> }> {
  const { publicKey, privateKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
  const jwk = { ...publicKey.export({ format: 'jwk' }), kid: 'test', alg: 'RS256', use: 'sig' };
  const miniflare = new Miniflare(
    convertV4MiniflareOptions({
      // Don't discover or register other local Wrangler/Miniflare sessions.
      unsafeDevRegistryPath: '',
      unsafeRegisterWorker: false,
      resourcePersistencePath: join(directory, 'state'),
      modules: true,
      script: await bundleService(),
      compatibilityDate: '2026-09-11',
      compatibilityFlags: ['nodejs_compat'],
      d1Databases: ['INDEX'],
      r2Buckets: ['ARTIFACTS'],
      bindings: {
        DEPLOYMENT_ID: 'local',
        NAMESPACES: JSON.stringify([namespace]),
        LIMITS: '{}',
        GC_BATCH_SIZE: '16',
        LOG_SAMPLE_RATE: '0',
      },
      ratelimits: {
        READ_LIMITER: { namespace_id: '1001', simple: { limit: 10000, period: 60 } },
        STORE_LIMITER: { namespace_id: '1002', simple: { limit: 10000, period: 60 } },
      },
      outboundService: (request) => {
        if (request.url !== `${issuer}/.well-known/jwks`) {
          throw new Error(`Unexpected outbound request to ${request.url}`);
        }
        return Response.json({ keys: [jwk] });
      },
    }),
  );

  try {
    const database = await miniflare.getD1Database('INDEX');
    await initializeDatabase(database);
    const blobs = new Blobs(directory, database, await miniflare.getR2Bucket('ARTIFACTS'));
    await blobs.replaceEdited();

    const server = createServer(async (request, response) => {
      const path = new URL(request.url ?? '/', 'http://localhost').pathname;
      const route = path.startsWith(basePath) ? path.slice(basePath.length) : path;
      let status = 500;
      let fields: ResponseFields = {};
      try {
        const headers: Record<string, string> = {};
        for (const [name, value] of Object.entries(request.headers)) {
          if (value !== undefined && !connectionHeaders.has(name)) headers[name] = String(value);
        }
        if (route === '/store')
          headers['authorization'] = `Bearer ${uploadToken(privateKey, jwk.kid)}`;
        const body = await readBody(request);
        const upstream = await miniflare.dispatchFetch(`${origin}${request.url}`, {
          method: request.method!,
          headers,
          ...(body.length > 0 ? { body } : {}),
        });
        const bytes = new Uint8Array(await upstream.arrayBuffer());
        status = upstream.status;
        response.writeHead(status, Object.fromEntries(upstream.headers));
        response.end(bytes);
        if (status === 200) fields = cborFields(upstream.headers.get('content-type'), bytes);
      } catch (error) {
        console.error(error);
        response.writeHead(status, { 'content-type': 'text/plain; charset=utf-8' });
        response.end('Internal server error');
      }
      if (route === '/store' && typeof fields.blob_id === 'string') blobs.add(fields.blob_id);
      const blobId = /^\/blob\/(.+)$/.exec(route)?.[1];
      const blobNumber = blobId === undefined ? undefined : blobs.find(blobId);
      const kind = route === '/fetch' && typeof fields.kind === 'string' ? fields.kind : undefined;
      const parts = [request.method, blobNumber ? `/blob/${blobNumber}` : route, status, kind];
      logRequest(parts.filter((part) => part !== undefined).join(' '));
    });
    server.listen(0, '127.0.0.1');
    await once(server, 'listening');
    const { port } = server.address() as AddressInfo;

    return {
      url: `http://127.0.0.1:${port}${basePath}`,
      async close() {
        server.close();
        server.closeAllConnections();
        await blobs.save();
        await miniflare.dispose();
      },
    };
  } catch (error) {
    await miniflare.dispose();
    throw error;
  }
}
