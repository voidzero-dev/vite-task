import assert from 'node:assert/strict';
import { appendFile, mkdir, writeFile } from 'node:fs/promises';
import { URL, pathToFileURL } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { encode } from 'cborg';
import {
  ApiError,
  operatorIO,
  query,
  readTemplate,
  runOperator,
  type Config,
  type OperatorIO,
} from './operator.ts';
import { retireTestData, seedManual, type Admin } from './e2e/fixtures.ts';
import { runSuite, type Report } from './e2e/suite.ts';

const resultsDir = new URL('../e2e-results/', import.meta.url);

export function settingsFrom(env: Record<string, string | undefined>) {
  const prefix = env['REMOTE_CACHE_RESOURCE_PREFIX'] || 'vp-cache-ci';
  if (!/^[a-z0-9][a-z0-9-]{0,30}-ci$/.test(prefix))
    throw new Error(
      'The resource prefix must end in -ci and contain at most 34 lowercase letters, digits, or hyphens',
    );
  const subdomain = env['REMOTE_CACHE_WORKERS_SUBDOMAIN'];
  if (!subdomain || !/^[a-z0-9][a-z0-9-]{0,62}$/.test(subdomain))
    throw new Error(
      'Set REMOTE_CACHE_WORKERS_SUBDOMAIN to the account subdomain, without workers.dev',
    );
  const pr = env['REMOTE_CACHE_PR_NUMBER'] || undefined;
  if (pr && !/^[1-9][0-9]{0,9}$/.test(pr)) throw new Error('Invalid PR number');
  const name = `${prefix}-${pr ? `pr-${pr}` : 'main'}`;
  const repository = env['GITHUB_REPOSITORY'];
  if (!repository || !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository))
    throw new Error('Invalid repository');
  const repositoryId = env['GITHUB_REPOSITORY_ID'];
  if (!repositoryId || !/^[1-9][0-9]*$/.test(repositoryId))
    throw new Error('Invalid repository ID');
  const revision = env['REMOTE_CACHE_SOURCE_SHA'];
  if (!revision || !/^[a-f0-9]{40}$/.test(revision))
    throw new Error('Use the full source commit SHA');
  const run = env['GITHUB_RUN_ID'],
    attempt = env['GITHUB_RUN_ATTEMPT'];
  if (!run || !attempt || !/^\d+$/.test(run) || !/^\d+$/.test(attempt))
    throw new Error('Invalid workflow run identity');
  const defaultBranch = env['REMOTE_CACHE_DEFAULT_BRANCH'];
  const writes =
    !pr &&
    env['GITHUB_EVENT_NAME'] === 'push' &&
    env['GITHUB_REF'] === `refs/heads/${defaultBranch}`;
  if (!pr && env['GITHUB_REF'] !== `refs/heads/${defaultBranch}`)
    throw new Error('The main staging deployment requires the default branch');
  return {
    name,
    pr,
    repository,
    repositoryId,
    revision,
    deployment: `${revision}-${run}-${attempt}`,
    origin: `https://${name}.${subdomain}.workers.dev`,
    writes,
    full: writes || env['REMOTE_CACHE_E2E_FULL'] === 'true',
  };
}
type Settings = ReturnType<typeof settingsFrom>;

function checkConfig(config: Config, settings: Settings) {
  if (
    config.name !== settings.name ||
    config.r2_buckets[0]?.bucket_name !== settings.name ||
    config.d1_databases[0]?.database_name !== settings.name
  )
    throw new Error('CI can only operate on its dedicated staging resources');
}

async function checkAccountOrigin(settings: Settings) {
  const account = (await operatorIO.api('/workers/subdomain')) as { subdomain: string };
  assert.equal(
    new URL(settings.origin).hostname,
    `${settings.name}.${account.subdomain}.workers.dev`,
    'The configured Workers subdomain must belong to the authenticated Cloudflare account',
  );
}

export function cloudflareAdmin(config: Config): Admin {
  const account = process.env['CLOUDFLARE_ACCOUNT_ID'],
    token = process.env['CLOUDFLARE_API_TOKEN'];
  if (!account || !/^[a-f0-9]{32}$/.test(account) || !token)
    throw new Error('Cloudflare staging credentials are required');
  const bucket = config.r2_buckets[0]!.bucket_name;
  // This is the authenticated object API used by the pinned Wrangler CLI.
  // Binary responses cannot pass through the operator's JSON API adapter.
  async function object(key: string, method: string, value?: Uint8Array) {
    if (!/^(e2e|other|manual)\/[a-f0-9-]{36}\/(value|blob)$/.test(key))
      throw new Error('Invalid fixture object key');
    const response = await fetch(
      `https://api.cloudflare.com/client/v4/accounts/${account}/r2/buckets/${bucket}/objects/${key}`,
      {
        method,
        headers: { Authorization: `Bearer ${token}` },
        ...(value === undefined ? {} : { body: value }),
        redirect: 'error',
        signal: AbortSignal.timeout(60000),
      },
    );
    await response.body?.cancel();
    if (!response.ok && response.status !== 404) throw new ApiError(response.status);
    return response.status !== 404;
  }
  return {
    sql: (sql, params = []) => query(operatorIO, config, sql, params),
    async put(key, value) {
      if (!(await object(key, 'PUT', value))) throw new Error('Fixture bucket is missing');
    },
    async delete(key) {
      await object(key, 'DELETE');
    },
    exists: (key) => object(key, 'GET'),
  };
}

export function githubTokens(
  env: Record<string, string | undefined>,
  request: typeof fetch = fetch,
) {
  const cached = new Map<string, { token: string; until: number }>();
  return async (audience: string) => {
    const previous = cached.get(audience);
    if (previous && previous.until > Date.now()) return previous.token;
    const raw = env['ACTIONS_ID_TOKEN_REQUEST_URL'],
      credential = env['ACTIONS_ID_TOKEN_REQUEST_TOKEN'];
    if (!raw || !credential) throw new Error('The e2e job requires id-token: write');
    const url = new URL(raw);
    if (
      url.protocol !== 'https:' ||
      !url.hostname.endsWith('.actions.githubusercontent.com') ||
      url.username ||
      url.password ||
      url.port
    )
      throw new Error('Unexpected GitHub OIDC request URL');
    url.searchParams.set('audience', audience);
    const response = await request(url, {
      headers: { Authorization: `Bearer ${credential}` },
      redirect: 'error',
      signal: AbortSignal.timeout(15000),
    });
    if (!response.ok) {
      await response.body?.cancel();
      throw new Error('GitHub OIDC request failed');
    }
    const reader = response.body!.getReader();
    const chunks: Uint8Array[] = [];
    let size = 0;
    try {
      while (true) {
        const part = await reader.read();
        if (part.done) break;
        size += part.value.length;
        if (size > 32768) throw new Error('OIDC response exceeds its limit');
        chunks.push(part.value);
      }
    } finally {
      await reader.cancel();
    }
    const data = JSON.parse(Buffer.concat(chunks).toString('utf8')) as { value?: unknown };
    if (typeof data.value !== 'string' || data.value.length > 16384)
      throw new Error('Invalid GitHub OIDC response');
    cached.set(audience, { token: data.value, until: Date.now() + 180000 });
    return data.value;
  };
}

async function deploy(settings: Settings) {
  await checkAccountOrigin(settings);
  await runOperator(
    [
      'setup',
      '--name',
      settings.name,
      '--namespace',
      'e2e',
      '--repo',
      settings.repository,
      '--origin',
      settings.origin,
      '--profile',
      'paid',
      '--retention-days',
      '1',
      '--byte-limit',
      '2000000000',
      '--entry-limit',
      '1000',
      '--association-limit',
      '2000',
    ],
    {
      ...operatorIO,
      async wrangler(args) {
        // Install all CI namespace policies before the single final deployment.
        if (args[0] !== 'deploy') await operatorIO.wrangler(args);
      },
      async writeConfig(config) {
        config.vars['DEPLOYMENT_ID'] = settings.deployment;
        await operatorIO.writeConfig(config);
      },
    },
  );
  const config = await operatorIO.readConfig();
  checkConfig(config, settings);
  const existing = await query(
    operatorIO,
    config,
    'SELECT scope_id, repository_id, endpoint FROM scopes',
  );
  if (
    existing.some(
      (scope) =>
        !['e2e', 'other', 'manual'].includes(String(scope['scope_id'])) ||
        scope['repository_id'] !== settings.repositoryId ||
        scope['endpoint'] !== `${settings.origin}/projects/${String(scope['scope_id'])}`,
    )
  )
    throw new Error('CI resources must contain only this repository’s verification namespaces');
  for (const scope of ['other', 'manual'])
    await query(
      operatorIO,
      config,
      `INSERT INTO scopes
    (scope_id, endpoint, repository, repository_id, repository_owner_id, branch, retention_seconds)
    SELECT ?, ?, repository, repository_id, repository_owner_id, branch, 86400 FROM scopes WHERE scope_id = 'e2e'
    ON CONFLICT(scope_id) DO NOTHING`,
      [scope, `${settings.origin}/projects/${scope}`],
    );
  // Recover policy changes left by an interrupted e2e run. These resources belong to CI only.
  await query(operatorIO, config, 'UPDATE deployment SET enabled = 1, writes_enabled = 1');
  await query(
    operatorIO,
    config,
    `UPDATE scopes SET enabled = 1, writes_enabled = 1,
    byte_limit = 2000000000, entry_limit = 1000, association_limit = 2000`,
  );
  config.vars['NAMESPACES'] = '["e2e","other","manual"]';
  await operatorIO.writeConfig(config);
  await operatorIO.wrangler(['deploy', '--config', 'wrangler.operator.json']);
  const managed = (await operatorIO.api(`/r2/buckets/${settings.name}/domains/managed`)) as {
    enabled: boolean;
  };
  const custom = (await operatorIO.api(`/r2/buckets/${settings.name}/domains/custom`)) as {
    domains: unknown[];
  };
  assert.equal(managed.enabled, false, 'R2 must remain private');
  assert.deepEqual(custom.domains, [], 'R2 must have no public custom domain');
  const fixture = await seedManual(cloudflareAdmin(config), settings.deployment);
  await mkdir(resultsDir, { recursive: true });
  await writeFile(
    new URL('manual-fetch.cbor', resultsDir),
    encode({ key: fixture.key, secondary_key: fixture.secondary }),
  );
  await writeFile(
    new URL('manual-manifest.json', resultsDir),
    JSON.stringify(
      {
        deployment: settings.deployment,
        source_sha: settings.revision,
        endpoint: `${settings.origin}/projects/manual`,
        blob_url: `${settings.origin}/projects/manual/blob/${fixture.blobId}`,
        value_utf8: Buffer.from(fixture.value).toString(),
        blob_utf8: Buffer.from(fixture.blob!).toString(),
      },
      null,
      2,
    ) + '\n',
  );
  if (process.env['GITHUB_OUTPUT'])
    await appendFile(
      process.env['GITHUB_OUTPUT'],
      `endpoint=${settings.origin}/projects/manual\ndeployment=${settings.deployment}\n`,
    );
}

async function verify(settings: Settings) {
  await checkAccountOrigin(settings);
  const config = await operatorIO.readConfig();
  checkConfig(config, settings);
  const admin = cloudflareAdmin(config);
  let report: Report | undefined;
  await mkdir(resultsDir, { recursive: true });
  try {
    await runSuite({
      origin: settings.origin,
      deployment: settings.deployment,
      admin,
      request: fetch,
      token: githubTokens(process.env),
      writes: settings.writes,
      full: settings.full,
      cron: settings.full,
      async record(value) {
        report = value;
        await writeFile(new URL('report.json', resultsDir), JSON.stringify(report, null, 2) + '\n');
      },
    });
  } finally {
    await retireTestData(admin);
    if (process.env['GITHUB_STEP_SUMMARY'] && report)
      await appendFile(
        process.env['GITHUB_STEP_SUMMARY'],
        `### Remote cache Cloudflare verification\n\nDeployment: \`${settings.deployment}\`\n\n` +
          `Mode: ${report.mode}. Manual endpoint: ${settings.origin}/projects/manual\n\n` +
          report.results.map((result) => `- ${result.status}: ${result.name}`).join('\n') +
          '\n',
      );
  }
}

export async function cleanup(
  settings: Settings,
  io: OperatorIO = operatorIO,
  sleep: (ms: number) => Promise<void> = delay,
) {
  if (!settings.pr) throw new Error('Automatic teardown is restricted to PR resources');
  const config = await readTemplate();
  const databases = (await io.api(`/d1/database?name=${settings.name}&per_page=100`)) as {
    name: string;
    uuid: string;
  }[];
  const db = databases.find((db) => db.name === settings.name);
  config.name = settings.name;
  config.r2_buckets[0]!.bucket_name = settings.name;
  config.d1_databases[0]!.database_name = settings.name;
  if (db) config.d1_databases[0]!.database_id = db.uuid;
  await io.writeConfig(config);
  if (db) {
    const tables = await query(
      io,
      config,
      "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'generations'",
    );
    if (tables.length) {
      const scopes = await query(io, config, 'SELECT repository_id, endpoint FROM scopes');
      if (
        scopes.some(
          (scope) =>
            scope['repository_id'] !== settings.repositoryId ||
            !String(scope['endpoint']).startsWith(`${settings.origin}/projects/`),
        )
      )
        throw new Error('Refusing to remove resources owned by another deployment');
      await query(io, config, 'UPDATE deployment SET enabled = 0, writes_enabled = 0');
      await query(
        io,
        config,
        `UPDATE generations SET expires_at = 0,
        lease_until = min(lease_until, unixepoch()), gc_after = min(gc_after, unixepoch() + 600)`,
      );
      const until = Date.now() + 25 * 60000;
      while (
        Number((await query(io, config, 'SELECT count(*) AS count FROM generations'))[0]!['count'])
      ) {
        if (Date.now() >= until)
          throw new Error(
            'Cleanup is still pending; rerun the cleanup workflow after Cron or lifecycle finishes',
          );
        await sleep(30000);
      }
    }
  }
  // Empty-bucket deletion also protects multipart uploads without a recorded ID.
  try {
    await io.api(`/r2/buckets/${settings.name}`);
    await io.wrangler(['r2', 'bucket', 'delete', settings.name]);
  } catch (error) {
    if (!(error instanceof ApiError) || error.status !== 404) throw error;
  }
  if (db) await io.wrangler(['d1', 'delete', db.uuid, '--skip-confirmation']);
  try {
    await io.api(`/workers/scripts/${settings.name}`, 'DELETE');
  } catch (error) {
    if (!(error instanceof ApiError) || error.status !== 404) throw error;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const settings = settingsFrom(process.env);
    const command = process.argv[2];
    if (command === 'deploy') await deploy(settings);
    else if (command === 'test') await verify(settings);
    else if (command === 'cleanup') await cleanup(settings);
    else throw new Error('Use deploy, test, or cleanup');
  } catch (error) {
    console.error(error instanceof Error ? error.message : 'Cloudflare CI failed');
    process.exitCode = 1;
  }
}
