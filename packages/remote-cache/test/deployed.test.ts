import assert from 'node:assert/strict';
import { test } from 'node:test';
import { harness } from './helpers.ts';
import { runSuite } from '../scripts/e2e/suite.ts';
import { seed, seedManual, bytes, type Admin } from '../scripts/e2e/fixtures.ts';
import { cleanup, githubTokens, settingsFrom } from '../scripts/ci.ts';
import { ApiError, readTemplate, type OperatorIO } from '../scripts/operator.ts';
import { randomUUID } from 'node:crypto';

for (const writes of [false, true])
  void test(`deployed HTTP suite runs against workerd with ${writes ? 'push' : 'PR'} claims`, async () => {
    const h = await harness({
      deploymentId: 'suite-local',
      namespaces: ['test', 'other', 'e2e', 'manual'],
    });
    try {
      for (const scope of ['e2e', 'manual'])
        await h.db
          .prepare(`INSERT INTO scopes
      (scope_id, endpoint, repository, repository_id, repository_owner_id, branch)
      VALUES (?, ?, 'owner/repo', '123', '456', 'refs/heads/main')`)
          .bind(scope, `https://cache.example.com/projects/${scope}`)
          .run();
      const admin: Admin = {
        async sql(sql, params = []) {
          if (sql.startsWith('SELECT generation_id FROM generations WHERE generation_id = ?'))
            await (await h.mf.getWorker()).scheduled();
          return (
            await h.db
              .prepare(sql)
              .bind(...params)
              .all()
          ).results;
        },
        async put(key, value) {
          await h.bucket.put(key, value);
        },
        async delete(key) {
          await h.bucket.delete(key);
        },
        async exists(key) {
          return (await h.bucket.head(key)) !== null;
        },
      };
      await seedManual(admin, 'suite-local');
      const report = await runSuite({
        origin: 'https://cache.example.com',
        deployment: 'suite-local',
        admin,
        writes,
        full: writes,
        cron: true,
        cronTimeoutMs: 5000,
        pollMs: 1,
        token: (aud) =>
          h.token({
            aud,
            ...(writes ? {} : { event_name: 'pull_request', ref: 'refs/pull/718/merge' }),
          }),
        async request(url, init) {
          // Serialize Node FormData before crossing Miniflare's fetch implementation.
          const request = new Request(url, init);
          const response = await h.mf.dispatchFetch(url, {
            method: request.method,
            headers: Object.fromEntries(request.headers),
            ...(request.method === 'GET' ? {} : { body: await request.arrayBuffer() }),
          });
          return new Response(await response.arrayBuffer(), {
            status: response.status,
            headers: response.headers,
          });
        },
      });
      assert.equal(report.results.length, writes ? 14 : 8);
      assert.ok(report.results.every((result) => result.status === 'passed'));
    } finally {
      await h.close();
    }
  });

void test('CI settings isolate PRs and only enable writes for default-branch pushes', () => {
  const env = {
    REMOTE_CACHE_WORKERS_SUBDOMAIN: 'example',
    GITHUB_REPOSITORY: 'owner/repo',
    GITHUB_REPOSITORY_ID: '123',
    REMOTE_CACHE_SOURCE_SHA: 'a'.repeat(40),
    GITHUB_RUN_ID: '42',
    GITHUB_RUN_ATTEMPT: '2',
    REMOTE_CACHE_DEFAULT_BRANCH: 'main',
    GITHUB_REF: 'refs/heads/main',
    GITHUB_EVENT_NAME: 'push',
  };
  assert.equal(settingsFrom(env).writes, true);
  assert.equal(settingsFrom({ ...env, GITHUB_EVENT_NAME: 'workflow_dispatch' }).writes, false);
  const pr = settingsFrom({
    ...env,
    REMOTE_CACHE_PR_NUMBER: '718',
    GITHUB_EVENT_NAME: 'pull_request',
  });
  assert.equal(pr.name, 'vp-cache-ci-pr-718');
  assert.equal(pr.writes, false);
  assert.throws(() => settingsFrom({ ...env, REMOTE_CACHE_RESOURCE_PREFIX: 'production' }));
  assert.throws(() => settingsFrom({ ...env, REMOTE_CACHE_PR_NUMBER: '718;rm -rf /' }));
  assert.throws(() => settingsFrom({ ...env, GITHUB_REF: 'refs/heads/feature' }));
});

void test('OIDC requests use only GitHub, reject redirects, and reuse tokens only for the same audience', async () => {
  const calls: { url: string; init?: RequestInit }[] = [];
  const request: typeof fetch = async (url, init) => {
    calls.push({ url: typeof url === 'string' ? url : 'href' in url ? url.href : url.url, init });
    return new Response(JSON.stringify({ value: 'signed-token' }));
  };
  const env = {
    ACTIONS_ID_TOKEN_REQUEST_URL: 'https://run-actions.example.actions.githubusercontent.com/token',
    ACTIONS_ID_TOKEN_REQUEST_TOKEN: 'runner-credential',
  };
  const token = githubTokens(env, request);
  await token('https://cache.example/projects/e2e');
  await token('https://cache.example/projects/e2e');
  await token('https://cache.example/projects/other');
  assert.equal(calls.length, 2);
  assert.equal(
    new URL(calls[0]!.url).searchParams.get('audience'),
    'https://cache.example/projects/e2e',
  );
  assert.equal(calls[0]!.init?.redirect, 'error');
  await assert.rejects(
    githubTokens(
      { ...env, ACTIONS_ID_TOKEN_REQUEST_URL: 'https://attacker.example/token' },
      request,
    )('audience'),
  );
  assert.equal(calls.length, 2);
});

void test('CI cleanup checks ownership, drains through Cron, preserves nonempty resources, and retries', async () => {
  const settings = settingsFrom({
    REMOTE_CACHE_WORKERS_SUBDOMAIN: 'example',
    REMOTE_CACHE_PR_NUMBER: '718',
    GITHUB_REPOSITORY: 'owner/repo',
    GITHUB_REPOSITORY_ID: '123',
    REMOTE_CACHE_SOURCE_SHA: 'a'.repeat(40),
    GITHUB_RUN_ID: '42',
    GITHUB_RUN_ATTEMPT: '1',
    REMOTE_CACHE_DEFAULT_BRANCH: 'main',
    GITHUB_REF: 'refs/heads/main',
    GITHUB_EVENT_NAME: 'workflow_dispatch',
  });
  const h = await harness();
  let config = await readTemplate();
  let database = true,
    bucket = true,
    worker = true;
  const deleted: string[] = [];
  const io: OperatorIO = {
    async api(path, method, body) {
      if (path.startsWith('/d1/database?'))
        return database ? [{ name: settings.name, uuid: 'test-db' }] : [];
      if (path.endsWith('/query')) {
        const { sql, params } = body as { sql: string; params: (string | number | null)[] };
        return [
          await h.db
            .prepare(sql)
            .bind(...params)
            .all(),
        ];
      }
      if (path.startsWith('/r2/')) {
        if (!bucket) throw new ApiError(404);
        return {};
      }
      if (path.startsWith('/workers/') && method === 'DELETE') {
        if (!worker) throw new ApiError(404);
        assert.equal(database, false);
        worker = false;
        deleted.push('worker');
        return null;
      }
      throw new Error('Unexpected Cloudflare API call');
    },
    async github() {
      throw new Error('Unexpected GitHub API call');
    },
    async lifecycle() {
      throw new Error('Unexpected lifecycle change');
    },
    async readConfig() {
      return config;
    },
    async writeConfig(value) {
      config = value;
    },
    print() {},
    async wrangler(args) {
      if (args[0] === 'r2') {
        if ((await h.bucket.list()).objects.length) throw new Error('Bucket is not empty');
        bucket = false;
        deleted.push('bucket');
      } else if (args[0] === 'd1') {
        assert.equal(bucket, false);
        database = false;
        deleted.push('database');
      } else throw new Error('Unexpected Wrangler command');
    },
  };
  try {
    await h.db
      .prepare("UPDATE scopes SET endpoint = ? || '/projects/' || scope_id")
      .bind(settings.origin)
      .run();
    const admin: Admin = {
      async sql(sql, params = []) {
        return (
          await h.db
            .prepare(sql)
            .bind(...params)
            .all()
        ).results;
      },
      async put(key, value) {
        await h.bucket.put(key, value);
      },
      async delete(key) {
        await h.bucket.delete(key);
      },
      async exists(key) {
        return (await h.bucket.head(key)) !== null;
      },
    };
    await seed(admin, 'other', 'cleanup', bytes('value'), bytes('blob'));
    const orphan = `other/${randomUUID()}/value`;
    await h.bucket.put(orphan, 'orphan');
    await assert.rejects(cleanup({ ...settings, repositoryId: '999' }, io), /another deployment/);
    assert.deepEqual(deleted, []);
    await assert.rejects(
      cleanup(settings, io, async () => {
        await h.db.prepare('UPDATE generations SET gc_after = 0').run();
        await (await h.mf.getWorker()).scheduled();
      }),
      /not empty/,
    );
    assert.equal(database, true);
    assert.equal(worker, true);
    assert.deepEqual(deleted, []);
    assert.equal(
      (await h.db.prepare('SELECT charged_bytes FROM deployment').first())!.charged_bytes,
      0,
    );
    await h.bucket.delete(orphan);
    await cleanup(settings, io);
    await cleanup(settings, io);
    assert.deepEqual(deleted, ['bucket', 'database', 'worker']);
    await assert.rejects(cleanup({ ...settings, pr: undefined }, io), /restricted to PR/);
  } finally {
    await h.close();
  }
});
