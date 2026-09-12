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

void test('operator setup is repeatable and policy changes preserve withdrawal', async () => {
  let config = await readTemplate();
  const h = await harness();
  const commands: string[][] = [];
  const output: string[] = [];
  const lifecycle: unknown[] = [];
  let failDeploy = false;
  const io: OperatorIO = {
    async api(path, _method, body) {
      if (path.startsWith('/d1/database?')) return [{ name: 'cache', uuid: 'test-database' }];
      if (path.endsWith('/query')) {
        const { sql, params } = body as { sql: string; params: unknown[] };
        return [
          await h.db
            .prepare(sql)
            .bind(...params)
            .all(),
        ];
      }
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
    async wrangler(args) {
      commands.push(args);
      if (failDeploy && args[0] === 'deploy') throw new Error('Deployment failed');
    },
    async readConfig() {
      return config;
    },
    async writeConfig(value) {
      config = value;
    },
    async lifecycle(_bucket, rules) {
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
    await runOperator(args, io);
    await runOperator(['policy', '--namespace', 'new', '--enabled', 'off', '--writes', 'off'], io);
    await runOperator(args, io);
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
