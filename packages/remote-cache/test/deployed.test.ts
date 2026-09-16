import assert from 'node:assert/strict';
import { test } from 'node:test';
import { harness } from './helpers.ts';
import { runSuite } from '../scripts/e2e/suite.ts';
import { retireTestData, seedManual, type Admin } from '../scripts/e2e/fixtures.ts';
import { githubTokens, settingsFrom } from '../scripts/ci.ts';

const modes = [
  { mode: 'read-only smoke', writes: false, full: false, checks: 8, runs: 2 },
  { mode: 'push smoke', writes: true, full: false, checks: 13, runs: 2 },
  { mode: 'push full', writes: true, full: true, checks: 14, runs: 1 },
];

for (const { mode, writes, full, checks, runs } of modes)
  void test(`deployed HTTP suite runs against workerd: ${mode}`, async () => {
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
      // Reuse storage for consecutive smoke runs, including previous test generations.
      for (let attempt = 0; attempt < runs; attempt++) {
        const manual = await seedManual(admin, 'suite-local');
        const report = await runSuite({
          origin: 'https://cache.example.com',
          deployment: 'suite-local',
          admin,
          writes,
          full,
          cron: full,
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
        assert.equal(report.results.length, checks);
        assert.ok(report.results.every((result) => result.status === 'passed'));
        assert.equal(
          report.results.some((result) => result.name.startsWith('real Cron')),
          full,
        );
        await retireTestData(admin);
        assert.ok(await admin.exists(manual.valueObject));
        assert.ok(await admin.exists(manual.blobObject));
        assert.equal(
          (
            await admin.sql('SELECT state FROM generations WHERE generation_id = ?', [
              manual.generation,
            ])
          )[0]?.['state'],
          'ready',
        );
      }
    } finally {
      await h.close();
    }
  });

void test('CI shares persistent staging across events and only permits writes on default-branch pushes', () => {
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
  const main = settingsFrom(env);
  assert.equal(main.name, 'vp-cache-ci-staging');
  assert.equal(main.origin, 'https://vp-cache-ci-staging.example.workers.dev');
  assert.equal(main.writes, true);
  for (const override of [
    { GITHUB_EVENT_NAME: 'workflow_dispatch' },
    { GITHUB_EVENT_NAME: 'pull_request', GITHUB_REF: 'refs/pull/718/merge' },
    { GITHUB_EVENT_NAME: 'pull_request', GITHUB_REF: 'refs/pull/719/merge' },
  ]) {
    const settings = settingsFrom({ ...env, ...override });
    assert.equal(settings.name, main.name);
    assert.equal(settings.origin, main.origin);
    assert.equal(settings.writes, false);
  }
  const retry = settingsFrom({ ...env, GITHUB_RUN_ATTEMPT: '3' });
  assert.equal(retry.origin, main.origin);
  assert.notEqual(retry.deployment, main.deployment);
  assert.equal(
    settingsFrom({ ...env, REMOTE_CACHE_RESOURCE_PREFIX: 'custom-ci' }).name,
    'custom-ci-staging',
  );
  assert.throws(() => settingsFrom({ ...env, REMOTE_CACHE_RESOURCE_PREFIX: 'production' }));
  for (const GITHUB_EVENT_NAME of ['push', 'workflow_dispatch'])
    assert.throws(() =>
      settingsFrom({ ...env, GITHUB_EVENT_NAME, GITHUB_REF: 'refs/heads/feature' }),
    );
  assert.throws(() => settingsFrom({ ...env, GITHUB_EVENT_NAME: 'pull_request_target' }));
});

void test('OIDC requests use only GitHub, reject redirects, and reuse tokens only for the same audience', async () => {
  const calls: { url: string; init?: RequestInit }[] = [];
  const request: typeof fetch = async (url, init) => {
    const requestUrl = url instanceof Request ? url.url : String(url);
    calls.push({ url: requestUrl, init });
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
