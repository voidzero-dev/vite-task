import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  runOperator,
  resolveRepository,
  lifecycleRules,
  readTemplate,
  ApiError,
  type OperatorIO,
} from '../scripts/operator.ts';
import { harness } from './helpers.ts';

void test('operator setup validates before writes and preserves withdrawal on retries', async () => {
  let config = await readTemplate();
  const h = await harness();
  const commands: string[][] = [];
  const output: string[] = [];
  const lifecycle: unknown[] = [];
  const writes: string[] = [];
  let repositoryId = 123;
  let failDeploy = false;
  const io: OperatorIO = {
    async api(path, _method, body) {
      if (path.startsWith('/d1/database?')) return [{ name: 'cache', uuid: 'test-database' }];
      if (path.endsWith('/query')) {
        const { sql, params } = body as { sql: string; params: unknown[] };
        if (!sql.startsWith('SELECT')) writes.push(sql);
        return [
          await h.db
            .prepare(sql)
            .bind(...params)
            .all(),
        ];
      }
      if (path.endsWith('/domains/custom')) return { domains: [] };
      if (_method && _method !== 'GET') writes.push(path);
      return {};
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
      writes.push(args.join(' '));
      commands.push(args);
      if (failDeploy && args[0] === 'deploy') throw new Error('Deployment failed');
    },
    async readConfig() {
      return config;
    },
    async writeConfig(value) {
      writes.push('config');
      config = structuredClone(value);
    },
    async lifecycle(_bucket, rules) {
      writes.push('lifecycle');
      lifecycle.push(rules);
    },
    print(message) {
      output.push(message);
    },
  };
  try {
    const args = [
      'setup',
      '--name',
      'cache',
      '--namespace',
      'new',
      '--repo',
      'owner/repo',
      '--origin',
      'https://cache.example.com',
    ];
    const originalConfig = structuredClone(config);
    const originalScopes = await h.db.prepare('SELECT * FROM scopes ORDER BY scope_id').all();
    const originalDeployment = await h.db.prepare('SELECT * FROM deployment').first();
    await assert.rejects(
      runOperator([...args.slice(0, -1), 'https://wrong.example.com', '--byte-limit', '1'], io),
      /different public origin/,
    );
    repositoryId = 999;
    await assert.rejects(
      runOperator(
        args.map((arg) => (arg === 'new' ? 'test' : arg)),
        io,
      ),
      /different repository/,
    );
    repositoryId = 123;
    assert.deepEqual(writes, []);
    assert.deepEqual(config, originalConfig);
    assert.deepEqual(
      (await h.db.prepare('SELECT * FROM scopes ORDER BY scope_id').all()).results,
      originalScopes.results,
    );
    assert.deepEqual(await h.db.prepare('SELECT * FROM deployment').first(), originalDeployment);
    await runOperator(args, io);
    const rateLimits = structuredClone(config.ratelimits);
    assert.ok(rateLimits.every((binding) => !['1001', '1002'].includes(binding.namespace_id)));
    await runOperator(['policy', '--namespace', 'new', '--enabled', 'off', '--writes', 'off'], io);
    await runOperator(args, io);
    assert.deepEqual(config.ratelimits, rateLimits);
    const row = await h.db.prepare("SELECT * FROM scopes WHERE scope_id = 'new'").first();
    assert.equal(row!.enabled, 0);
    assert.equal(row!.writes_enabled, 0);
    assert.equal(row!.policy_version, 2);
    assert.equal(config.workers_dev, false);
    assert.equal(config.preview_urls, false);
    assert.deepEqual(config.routes, [{ pattern: 'cache.example.com', custom_domain: true }]);
    assert.equal(
      JSON.parse(config.vars.NAMESPACES!).filter((name: string) => name === 'new').length,
      1,
    );
    assert.equal(
      commands.some((args) => args.includes('create')),
      false,
    );
    assert.equal(lifecycle.length, 2);
    assert.equal(output.at(-1), 'https://cache.example.com/projects/new');
    assert.deepEqual(lifecycleRules(30), {
      rules: [
        {
          id: 'remote-cache-generations',
          enabled: true,
          conditions: { prefix: '' },
          deleteObjectsTransition: { condition: { type: 'Age', maxAge: 32 * 86400 } },
          abortMultipartUploadsTransition: { condition: { type: 'Age', maxAge: 86400 } },
        },
      ],
    });
    await assert.rejects(runOperator(['teardown'], io), /confirm/);
    await assert.rejects(runOperator(['purge', '--namespace', 'test'], io), /confirm/);
    failDeploy = true;
    const bind = ['bind', '--namespace', 'retry', '--repo', 'owner/repo'];
    await assert.rejects(runOperator(bind, io), /Deployment failed/);
    const attempts = commands.length;
    failDeploy = false;
    await runOperator(bind, io);
    assert.equal(commands.length, attempts + 1);
    assert.equal(commands.at(-1)![0], 'deploy');
    assert.ok(JSON.parse(config.vars.NAMESPACES!).includes('retry'));
  } finally {
    await h.close();
  }
});

void test('operator upgrades isolate rate counters by Worker and retain them across revisions', async () => {
  const namespaces = new Set<string>();
  for (const name of ['vp-cache-ci-pr-1', 'vp-cache-ci-pr-2', 'vp-cache-ci-main', 'production']) {
    let config = await readTemplate();
    config.name = name;
    const limits = config.ratelimits.map((binding) => binding.simple);
    const deployed: (typeof config)[] = [];
    const io: OperatorIO = {
      async api() {
        return [{ success: true, results: [{ retention: 604800 }] }];
      },
      async github() {
        throw new Error('Unexpected GitHub request');
      },
      async readConfig() {
        return structuredClone(config);
      },
      async writeConfig(value) {
        config = structuredClone(value);
      },
      async wrangler(args) {
        if (args[0] === 'deploy') deployed.push(structuredClone(config));
      },
      async lifecycle() {},
      print() {},
    };
    await runOperator(['upgrade'], io);
    for (const binding of config.ratelimits) {
      assert.match(binding.namespace_id, /^[1-9][0-9]*$/);
      assert.ok(Number.isSafeInteger(Number(binding.namespace_id)));
      assert.ok(
        !namespaces.has(binding.namespace_id),
        'deployments and bindings must not share counters',
      );
      namespaces.add(binding.namespace_id);
    }
    assert.deepEqual(
      config.ratelimits.map((binding) => binding.simple),
      limits,
    );
    config.vars['DEPLOYMENT_ID'] = 'next-revision';
    await runOperator(['upgrade'], io);
    assert.equal(deployed.length, 2);
    assert.deepEqual(deployed[0]!.ratelimits, deployed[1]!.ratelimits);
  }
});

void test('operator setup creates resources and initializes an empty database', async () => {
  const h = await harness({ initializeDatabase: false });
  let config = await readTemplate();
  let database = false;
  let bucket = false;
  let deployed = false;
  const io: OperatorIO = {
    async api(path, _method, body) {
      if (path.startsWith('/d1/database?'))
        return database ? [{ name: 'cache', uuid: 'test-database' }] : [];
      if (path.endsWith('/query')) {
        const { sql, params } = body as { sql: string; params: unknown[] };
        return [
          await h.db
            .prepare(sql)
            .bind(...params)
            .all(),
        ];
      }
      if (path === '/r2/buckets/cache' && !bucket) throw new ApiError(404);
      if (path.endsWith('/domains/custom')) return { domains: [] };
      return {};
    },
    async github() {
      return {
        id: 123,
        full_name: 'owner/repo',
        owner: { id: 456 },
        private: false,
        visibility: 'public',
        default_branch: 'main',
      };
    },
    async readConfig() {
      return structuredClone(config);
    },
    async writeConfig(value) {
      config = structuredClone(value);
    },
    async wrangler(args) {
      if (args[0] === 'd1' && args[1] === 'create') database = true;
      if (args[0] === 'r2' && args[2] === 'create') bucket = true;
      if (args[1] === 'migrations') await h.migrate();
      if (args[0] === 'deploy') deployed = true;
    },
    async lifecycle() {},
    print() {},
  };
  try {
    await runOperator(
      [
        'setup',
        '--name',
        'cache',
        '--namespace',
        'test',
        '--repo',
        'owner/repo',
        '--origin',
        'https://cache.example.com',
      ],
      io,
    );
    assert.ok(database && bucket && deployed);
    assert.deepEqual(JSON.parse(config.vars['NAMESPACES']!), ['test']);
    const scope = await h.db.prepare("SELECT endpoint FROM scopes WHERE scope_id = 'test'").first();
    assert.equal(scope!.endpoint, 'https://cache.example.com/projects/test');
  } finally {
    await h.close();
  }
});

void test('teardown resumes after partial resource deletion', async () => {
  const config = await readTemplate();
  let database = true,
    bucket = true,
    failDatabase = true,
    failWorker = true;
  const deleted: string[] = [];
  const io: OperatorIO = {
    async api(path) {
      if (path.endsWith('/query')) return [{ success: true, results: [{ count: 0 }] }];
      if ((path.startsWith('/d1/') && !database) || (path.startsWith('/r2/') && !bucket))
        throw new ApiError(404);
      return {};
    },
    async github() {
      throw new Error('Unexpected GitHub request');
    },
    async readConfig() {
      return config;
    },
    async writeConfig() {
      throw new Error('Unexpected config write');
    },
    async lifecycle() {
      throw new Error('Unexpected lifecycle write');
    },
    print() {},
    async wrangler(args) {
      if (args[0] === 'r2') {
        bucket = false;
        deleted.push('bucket');
      } else if (args[0] === 'd1') {
        if (failDatabase) {
          failDatabase = false;
          throw new Error('D1 deletion failed');
        }
        database = false;
        deleted.push('database');
      } else {
        if (failWorker) {
          failWorker = false;
          throw new Error('Worker deletion failed');
        }
        deleted.push('worker');
      }
    },
  };
  const args = ['teardown', '--confirm', config.name];
  await assert.rejects(runOperator(args, io), /D1 deletion failed/);
  await assert.rejects(runOperator(args, io), /Worker deletion failed/);
  await runOperator(args, io);
  assert.deepEqual(deleted, ['bucket', 'database', 'worker']);
});

void test('operator rejects private repositories and injection-shaped names', async () => {
  const io = {
    github: async () => ({ id: 123, owner: { id: 456 }, private: true, visibility: 'private' }),
  };
  await assert.rejects(resolveRepository(io, 'owner/repo'), /public/);
  await assert.rejects(
    resolveRepository(io, "owner/repo'; DROP TABLE scopes;"),
    /owner\/repository/,
  );
});
