import assert from 'node:assert/strict';
import { test } from 'node:test';
import { deploy, checkDeployment } from '../scripts/deploy.ts';
import { ApiError, readTemplate, type OperatorIO } from '../scripts/operator.ts';
import { harness } from './helpers.ts';

void test('button deployment uses provisioned bindings and preserves policies on a fresh-checkout retry', async () => {
  const h = await harness({
    initializeDatabase: false,
    namespaces: ['cache'],
    deploymentId: 'button-test',
  });
  const template = await readTemplate();
  template.name = 'custom-worker';
  template.vars['CACHE_REPOSITORY'] = 'owner/repo';
  template.d1_databases[0]!.database_id = '12345678-1234-1234-1234-123456789abc';
  template.d1_databases[0]!.database_name = 'custom-index';
  template.r2_buckets[0]!.bucket_name = 'custom-artifacts';
  let config = structuredClone(template);
  let migrated = false;
  let repositoryId = 123;
  let bucketMissing = false;
  let staleResponse = true;
  const commands: string[][] = [];
  const mutations: string[] = [];
  const waits: number[] = [];
  const io: OperatorIO = {
    async api(path, method, body) {
      if (path === '/workers/subdomain') return { subdomain: 'team' };
      if (path === `/d1/database/${template.d1_databases[0]!.database_id}`)
        return { uuid: template.d1_databases[0]!.database_id, name: 'custom-index' };
      if (path.endsWith('/query')) {
        assert.ok(path.includes(template.d1_databases[0]!.database_id));
        const { sql, params } = body as { sql: string; params: (string | number | null)[] };
        if (!sql.startsWith('SELECT')) mutations.push(sql);
        return [
          await h.db
            .prepare(sql)
            .bind(...params)
            .all(),
        ];
      }
      if (path === '/r2/buckets/custom-artifacts') {
        if (bucketMissing) throw new ApiError(404);
        return { storage_class: 'Standard' };
      }
      if (path === '/r2/buckets/custom-artifacts/domains/custom') return { domains: [] };
      if (path === '/r2/buckets/custom-artifacts/domains/managed') {
        assert.equal(method, 'PUT');
        assert.deepEqual(body, { enabled: false });
        mutations.push('disable public bucket');
        return {};
      }
      throw new Error(`Unexpected API request: ${path}`);
    },
    async github() {
      return {
        id: repositoryId,
        full_name: 'owner/repo',
        owner: { id: 456 },
        private: false,
        visibility: 'public',
        default_branch: 'main',
      };
    },
    async wrangler(args) {
      assert.ok(!args.includes('create'), 'Provisioned resources must not be recreated');
      commands.push(args);
      if (args[1] === 'migrations' && !migrated) {
        await h.migrate();
        migrated = true;
      }
    },
    async readConfig() {
      return structuredClone(config);
    },
    async writeConfig(value) {
      config = structuredClone(value);
      mutations.push('config');
    },
    async lifecycle(bucket) {
      assert.equal(bucket, 'custom-artifacts');
      mutations.push('lifecycle');
    },
    print() {},
  };
  const request: typeof fetch = async (input, init) => {
    if (staleResponse) {
      staleResponse = false;
      return new Response(null, {
        status: 401,
        headers: { 'X-Remote-Cache-Deployment': 'old', 'Cache-Control': 'no-store' },
      });
    }
    const req = new Request(input, init);
    const response = await h.mf.dispatchFetch(req.url, {
      method: req.method,
      headers: Object.fromEntries(req.headers),
      body: await req.arrayBuffer(),
    });
    return new Response(await response.arrayBuffer(), {
      status: response.status,
      headers: Object.fromEntries(response.headers),
    });
  };
  const run = (profile = 'free') =>
    deploy(
      template,
      io,
      { WORKERS_CI_BUILD_UUID: 'button-test', CACHE_PROFILE: profile },
      request,
      async (ms) => {
        waits.push(ms);
      },
    );
  try {
    await run();
    assert.deepEqual(waits, [3000]);
    assert.equal(config.name, 'custom-worker');
    assert.equal(config.d1_databases[0]!.database_name, 'custom-index');
    assert.equal(config.r2_buckets[0]!.bucket_name, 'custom-artifacts');
    assert.equal(config.vars['GC_BATCH_SIZE'], '16');
    assert.equal(config.vars['DEPLOYMENT_ID'], 'button-test');
    assert.deepEqual(JSON.parse(config.vars['NAMESPACES']!), ['cache']);
    assert.equal(commands.at(-1)![0], 'deploy');
    const scope = await h.db.prepare('SELECT * FROM scopes').first();
    assert.equal(scope!.endpoint, 'https://custom-worker.team.workers.dev/projects/cache');
    assert.equal(scope!.repository_id, '123');
    assert.equal(scope!.branch, 'refs/heads/main');

    await h.db
      .prepare(
        "UPDATE scopes SET enabled = 0, writes_enabled = 0, byte_limit = 12345 WHERE scope_id = 'cache'",
      )
      .run();
    const withdrawn = await h.db.prepare('SELECT * FROM scopes').first();
    config = structuredClone(template); // A new Workers Build has no wrangler.operator.json.
    await run('paid');
    assert.equal(config.vars['GC_BATCH_SIZE'], '256');
    assert.deepEqual(await h.db.prepare('SELECT * FROM scopes').first(), withdrawn);

    mutations.length = 0;
    repositoryId = 999;
    await assert.rejects(run(), /different repository/);
    assert.deepEqual(mutations, []);
    repositoryId = 123;
    bucketMissing = true;
    await assert.rejects(run(), /Cloudflare API failed/);
    assert.deepEqual(mutations, []);
  } finally {
    await h.close();
  }
});

void test('button deployment rejects incomplete configuration before contacting Cloudflare', async () => {
  const config = await readTemplate();
  const unexpected = async () => {
    throw new Error('Unexpected operator call');
  };
  const io: OperatorIO = {
    api: unexpected,
    github: unexpected,
    wrangler: unexpected,
    readConfig: unexpected,
    writeConfig: unexpected,
    lifecycle: unexpected,
    print() {
      throw new Error('Unexpected output');
    },
  };
  await assert.rejects(deploy(config, io, {}), /CACHE_REPOSITORY/);
  config.vars['CACHE_REPOSITORY'] = 'owner/repo';
  await assert.rejects(deploy(config, io, {}), /provisioned/);
  config.d1_databases[0]!.database_id = '12345678-1234-1234-1234-123456789abc';
  await assert.rejects(
    deploy(config, io, { WRANGLER_CI_OVERRIDE_NAME: 'different-worker' }),
    /Worker name/,
  );
});

void test('deployment checks reject a stale revision, unexpected status, or cached response', async () => {
  for (const response of [
    new Response(null, {
      status: 401,
      headers: { 'X-Remote-Cache-Deployment': 'old', 'Cache-Control': 'no-store' },
    }),
    new Response(null, {
      status: 200,
      headers: { 'X-Remote-Cache-Deployment': 'new', 'Cache-Control': 'no-store' },
    }),
    new Response(null, { status: 401, headers: { 'X-Remote-Cache-Deployment': 'new' } }),
  ]) {
    await assert.rejects(
      checkDeployment('https://cache.example.com/projects/test', 'new', true, async () => response),
      /Deployment check failed/,
    );
  }
});
