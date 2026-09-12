import { readFile } from 'node:fs/promises';
import { fileURLToPath, URL } from 'node:url';
import { build } from 'esbuild';
import { Miniflare, convertV4MiniflareOptions } from 'miniflare';
import { exportJWK, generateKeyPair, SignJWT } from 'jose';
import { encode, decode } from 'cborg';
import { ISSUER, JWKS_URL } from '../src/auth.ts';

export const endpoint = 'https://cache.example.com/projects/test';
export const bytes = (value: string) => new TextEncoder().encode(value);
export const decodeResponse = async (response: Pick<Response, 'arrayBuffer'>) =>
  decode(new Uint8Array(await response.arrayBuffer()));

export async function harness(
  options: {
    readLimit?: number;
    storeLimit?: number;
    jwksStatus?: number;
    limits?: Record<string, number>;
    inspector?: boolean;
  } = {},
) {
  const pair = await generateKeyPair('RS256', { extractable: true });
  const jwk = { ...(await exportJWK(pair.publicKey)), kid: 'test-key', alg: 'RS256', use: 'sig' };
  const bundle = await build({
    entryPoints: [fileURLToPath(new URL('../src/index.ts', import.meta.url))],
    bundle: true,
    write: false,
    format: 'esm',
    platform: 'neutral',
    target: 'es2022',
    external: ['node:*', 'cloudflare:*'],
  });
  let jwksRequests = 0;
  const mf = new Miniflare(
    convertV4MiniflareOptions({
      // Tests must not discover or restart other local Wrangler/Miniflare sessions.
      unsafeDevRegistryPath: '',
      unsafeRegisterWorker: false,
      ...(options.inspector ? { inspectorPort: 0 } : {}),
      modules: true,
      script: bundle.outputFiles[0]!.text,
      compatibilityDate: '2026-09-11',
      compatibilityFlags: ['nodejs_compat'],
      d1Databases: ['INDEX'],
      r2Buckets: ['ARTIFACTS'],
      bindings: {
        NAMESPACES: '["test","other"]',
        LIMITS: JSON.stringify(options.limits ?? {}),
        GC_BATCH_SIZE: '16',
        LOG_SAMPLE_RATE: '0',
      },
      ratelimits: {
        READ_LIMITER: {
          namespace_id: '1001',
          simple: { limit: options.readLimit ?? 10000, period: 60 },
        },
        STORE_LIMITER: {
          namespace_id: '1002',
          simple: { limit: options.storeLimit ?? 10000, period: 60 },
        },
      },
      outboundService: async (request) => {
        if (request.url !== JWKS_URL) throw new Error('Unexpected outbound request');
        jwksRequests++;
        return new Response(JSON.stringify({ keys: [jwk] }), {
          status: options.jwksStatus ?? 200,
          headers: { 'Content-Type': 'application/json' },
        });
      },
    }),
  );
  try {
    const db = await mf.getD1Database('INDEX');
    const bucket = await mf.getR2Bucket('ARTIFACTS');
    const migration = await readFile(
      new URL('../migrations/0001_cache.sql', import.meta.url),
      'utf8',
    );
    // Preserve complete trigger bodies. Each prepared statement is a real D1 migration statement.
    const statements = migration.match(
      /CREATE TRIGGER[\s\S]*?\nEND;|(?:CREATE TABLE|CREATE (?:UNIQUE )?INDEX|INSERT INTO)[\s\S]*?;/g,
    )!;
    await db.batch(statements.map((sql) => db.prepare(sql)));
    for (const name of ['test', 'other'])
      await db
        .prepare(`INSERT INTO scopes (scope_id, endpoint, repository, repository_id, repository_owner_id, branch)
    VALUES (?, ?, 'owner/repo', '123', '456', 'refs/heads/main')`)
        .bind(name, `https://cache.example.com/projects/${name}`)
        .run();
    async function token(claims: Record<string, unknown> = {}, kid = 'test-key') {
      const now = Math.floor(Date.now() / 1000);
      return new SignJWT({
        iss: ISSUER,
        aud: endpoint,
        repository_id: '123',
        repository_owner_id: '456',
        repository_visibility: 'public',
        ref: 'refs/heads/main',
        ref_type: 'branch',
        event_name: 'push',
        exp: now + 300,
        iat: now,
        nbf: now - 1,
        ...claims,
      })
        .setProtectedHeader({ alg: 'RS256', kid })
        .sign(pair.privateKey);
    }
    async function store(
      key: Uint8Array,
      secondary: Uint8Array,
      value: Uint8Array,
      blob?: Uint8Array,
      options: { token?: string; scope?: string; blobFirst?: boolean } = {},
    ) {
      const form = new FormData();
      const addBlob = () => {
        if (blob !== undefined)
          form.append('blob', new Blob([blob], { type: 'application/octet-stream' }), 'blob');
      };
      if (options.blobFirst) addBlob();
      form.append(
        'metadata',
        new Blob([encode({ key, secondary_key: secondary, value })], { type: 'application/cbor' }),
        'metadata',
      );
      if (!options.blobFirst) addBlob();
      const request = new Request(
        `https://cache.example.com/projects/${options.scope ?? 'test'}/store`,
        {
          method: 'POST',
          headers: { Authorization: `Bearer ${options.token ?? (await token())}` },
          body: form,
        },
      );
      return mf.dispatchFetch(request.url, {
        method: 'POST',
        headers: Object.fromEntries(request.headers),
        body: await request.arrayBuffer(),
      });
    }
    function fetch(key: Uint8Array, secondary: Uint8Array, scope = 'test') {
      return mf.dispatchFetch(`https://cache.example.com/projects/${scope}/fetch`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/cbor' },
        body: encode({ key, secondary_key: secondary }),
      });
    }
    return {
      mf,
      db,
      bucket,
      token,
      store,
      fetch,
      jwksRequests: () => jwksRequests,
      close: () => mf.dispose(),
    };
  } catch (error) {
    await mf.dispose();
    throw error;
  }
}
