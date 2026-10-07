import { build } from 'esbuild';
import { Miniflare, convertV4MiniflareOptions } from 'miniflare';
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { unstable_getMiniflareWorkerOptions, unstable_splitSqlQuery } from 'wrangler';
import { issuer, type repository, type TrustedKey } from './github.ts';

const serviceDirectory = fileURLToPath(
  new URL(
    './',
    import.meta.resolve('@voidzero-dev/vite-plus-remote-cache-cloudflare/package.json'),
  ),
);

/** A running backend. */
export interface Backend {
  /** Where the backend listens, e.g. `http://127.0.0.1:1234`, without a path. */
  origin: string;
  /** Replace the contents of the blob with ID `id`. */
  writeBlob(id: string, contents: Uint8Array): Promise<void>;
  close(): Promise<void>;
}

/** The Worker in one module, bundled from the source as for its own tests. */
async function bundle(main: string): Promise<string> {
  const result = await build({
    entryPoints: [main],
    bundle: true,
    write: false,
    format: 'esm',
    platform: 'neutral',
    target: 'es2022',
    external: ['node:*', 'cloudflare:*'],
  });
  return result.outputFiles[0]!.text;
}

/**
 * Start Vite+'s Cloudflare remote cache, from
 * voidzero-dev/vite-plus-remote-cache-cloudflare, in workerd on a free loopback
 * port, with its D1 database and R2 bucket in `directory/state`.
 * The Worker runs with the compatibility settings, bindings, and limits in its
 * `wrangler.jsonc`. Its namespaces are only the one in `basePath`.
 *
 * Like a deployment, the database has every migration applied, and the
 * namespace is registered for `endpoint` and the repository `registered`. The
 * Worker fetches GitHub's key set to verify upload tokens, and gets one with
 * `key` instead. Any other outbound request fails.
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
  const namespace = basePath.slice(basePath.lastIndexOf('/') + 1);
  const { workerOptions, main } = unstable_getMiniflareWorkerOptions(
    join(serviceDirectory, 'wrangler.jsonc'),
  );
  // Module rules only apply to modules loaded from files, not to one script.
  const { modulesRules: _, ...options } = workerOptions;
  const miniflare = new Miniflare(
    convertV4MiniflareOptions({
      ...options,
      // Don't discover or register other local Wrangler or Miniflare sessions.
      unsafeDevRegistryPath: '',
      unsafeRegisterWorker: false,
      host: '127.0.0.1',
      port: 0,
      resourcePersistencePath: join(directory, 'state'),
      modules: true,
      script: await bundle(main!),
      bindings: {
        ...(options['bindings'] as Record<string, unknown>),
        NAMESPACES: JSON.stringify([namespace]),
      },
      outboundService: (request) => {
        if (request.url !== `${issuer}/.well-known/jwks`) {
          throw new Error(`Unexpected outbound request to ${request.url}`);
        }
        return Response.json({ keys: [key.jwk] });
      },
    }),
  );

  try {
    const origin = (await miniflare.ready).origin;
    const database = await miniflare.getD1Database('INDEX');
    const migrations = join(serviceDirectory, 'migrations');
    for (const file of readdirSync(migrations)
      .filter((name) => name.endsWith('.sql'))
      .sort()) {
      const statements = unstable_splitSqlQuery(readFileSync(join(migrations, file), 'utf8'));
      await database.batch(statements.map((sql) => database.prepare(sql)));
    }
    await database
      .prepare(
        `INSERT INTO scopes (scope_id, endpoint, repository, repository_id, repository_owner_id, branch)
        VALUES (?, ?, ?, ?, ?, ?)`,
      )
      .bind(
        namespace,
        endpoint,
        registered.name,
        registered.id,
        registered.ownerId,
        registered.branch,
      )
      .run();
    const bucket = await miniflare.getR2Bucket('ARTIFACTS');

    return {
      origin,
      async writeBlob(id, contents) {
        const object = await database
          .prepare('SELECT blob_object FROM generations WHERE blob_id = ?')
          .bind(id)
          .first<string>('blob_object');
        if (object === null) throw new Error(`No blob ${id} is stored`);
        await bucket.put(object, contents);
        // The Worker checks the size of each blob it serves.
        await database
          .prepare('UPDATE generations SET blob_size = ? WHERE blob_id = ?')
          .bind(contents.length, id)
          .run();
      },
      close: () => miniflare.dispose(),
    };
  } catch (error) {
    await miniflare.dispose();
    throw error;
  }
}
