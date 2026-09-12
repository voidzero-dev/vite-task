import { spawn } from 'node:child_process';
import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath, pathToFileURL, URL } from 'node:url';
import { createRequire } from 'node:module';
import { parseArgs } from 'node:util';
import { parse, type ParseError } from 'jsonc-parser';

const root = fileURLToPath(new URL('../', import.meta.url));
const configPath = join(root, 'wrangler.operator.json');
const require = createRequire(import.meta.url);

export async function readTemplate(): Promise<Config> {
  const errors: ParseError[] = [];
  const config = parse(await readFile(join(root, 'wrangler.jsonc'), 'utf8'), errors, {
    allowTrailingComma: true,
  });
  if (errors.length) throw new Error('Invalid Wrangler JSONC template');
  return config;
}

export interface OperatorIO {
  api(path: string, method?: string, body?: unknown): Promise<unknown>;
  github(repository: string): Promise<unknown>;
  wrangler(args: string[]): Promise<void>;
  readConfig(): Promise<Config>;
  writeConfig(config: Config): Promise<void>;
  lifecycle(bucket: string, rules: unknown): Promise<void>;
  print(message: string): void;
}
export type Config = {
  name: string;
  main: string;
  compatibility_date: string;
  compatibility_flags: string[];
  workers_dev: boolean;
  preview_urls: boolean;
  routes?: { pattern: string; custom_domain: boolean }[];
  observability: { enabled: boolean; head_sampling_rate: number };
  triggers: { crons: string[] };
  d1_databases: {
    binding: string;
    database_name: string;
    database_id: string;
    migrations_dir: string;
  }[];
  r2_buckets: { binding: string; bucket_name: string }[];
  ratelimits: { name: string; namespace_id: string; simple: { limit: number; period: number } }[];
  vars: Record<string, string>;
};

function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new Error('Invalid API response');
  return value as Record<string, unknown>;
}
function text(value: unknown): string {
  if (typeof value !== 'string' || !value) throw new Error('Expected a nonempty string');
  return value;
}
function positive(value: string | undefined, fallback: number): number {
  const number = value === undefined ? fallback : Number(value);
  if (!Number.isSafeInteger(number) || number <= 0) throw new Error('Expected a positive integer');
  return number;
}
function toggle(value: string | undefined): number | null {
  if (value === undefined) return null;
  if (value !== 'on' && value !== 'off') throw new Error('Use on or off');
  return Number(value === 'on');
}
function identifier(value: string | undefined): string {
  if (!value || !/^[a-z0-9][a-z0-9-]{0,62}$/.test(value))
    throw new Error('Use a lowercase name with letters, digits, and hyphens (1–63 characters)');
  return value;
}

export async function resolveRepository(io: Pick<OperatorIO, 'github'>, name: string) {
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(name)) throw new Error('Use owner/repository');
  const repo = object(await io.github(name));
  if (repo['private'] !== false || repo['visibility'] !== 'public')
    throw new Error('Only public repositories can publish');
  const owner = object(repo['owner']);
  if (!Number.isSafeInteger(repo['id']) || !Number.isSafeInteger(owner['id']))
    throw new Error('Invalid GitHub IDs');
  return {
    name: text(repo['full_name']),
    id: String(repo['id']),
    owner: String(owner['id']),
    branch: `refs/heads/${text(repo['default_branch'])}`,
  };
}

export async function query(
  io: OperatorIO,
  config: Config,
  sql: string,
  params: (string | number | null)[] = [],
) {
  const result = await io.api(`/d1/database/${config.d1_databases[0]!.database_id}/query`, 'POST', {
    sql,
    params,
  });
  if (!Array.isArray(result) || result.some((row) => object(row)['success'] !== true))
    throw new Error('D1 query failed');
  return result.flatMap((row) => {
    const rows = object(row)['results'];
    if (!Array.isArray(rows)) throw new Error('Invalid D1 results');
    return rows.map(object);
  });
}

async function exists(io: OperatorIO, path: string): Promise<boolean> {
  try {
    await io.api(path);
    return true;
  } catch (error) {
    if (error instanceof ApiError && error.status === 404) return false;
    throw error;
  }
}

export function lifecycleRules(retentionDays: number) {
  return {
    rules: [
      {
        id: 'remote-cache-generations',
        enabled: true,
        conditions: { prefix: '' },
        deleteObjectsTransition: {
          condition: { type: 'Age', maxAge: (retentionDays + 2) * 86400 },
        },
        abortMultipartUploadsTransition: { condition: { type: 'Age', maxAge: 86400 } },
      },
    ],
  };
}

async function updateLifecycle(io: OperatorIO, config: Config) {
  const rows = await query(
    io,
    config,
    'SELECT retention_high_water_seconds AS retention FROM deployment',
  );
  const days = Math.ceil(Number(rows[0]?.['retention'] ?? 604800) / 86400);
  await io.lifecycle(config.r2_buckets[0]!.bucket_name, lifecycleRules(days));
}

export async function runOperator(argv: string[], io: OperatorIO): Promise<void> {
  const { values, positionals } = parseArgs({
    args: argv,
    allowPositionals: true,
    options: {
      name: { type: 'string' },
      namespace: { type: 'string' },
      repo: { type: 'string' },
      origin: { type: 'string' },
      profile: { type: 'string' },
      'retention-days': { type: 'string' },
      'byte-limit': { type: 'string' },
      'entry-limit': { type: 'string' },
      'association-limit': { type: 'string' },
      enabled: { type: 'string' },
      writes: { type: 'string' },
      confirm: { type: 'string' },
      help: { type: 'boolean' },
    },
  });
  const command = positionals[0];
  if (values.help || !command) {
    io.print(
      'Commands: setup, bind, policy, deployment, status, upgrade, purge, teardown. See README.md for options.',
    );
    return;
  }
  if (
    !['setup', 'bind', 'policy', 'deployment', 'status', 'upgrade', 'purge', 'teardown'].includes(
      command,
    )
  )
    throw new Error('Unknown command');
  for (const name of [
    'retention-days',
    'byte-limit',
    'entry-limit',
    'association-limit',
  ] as const) {
    if (values[name] !== undefined) positive(values[name], 1);
  }
  if (values['retention-days'] && positive(values['retention-days'], 7) > 365)
    throw new Error('Retention cannot exceed 365 days');
  toggle(values.enabled);
  toggle(values.writes);
  let config: Config;
  if (command === 'setup') {
    const name = identifier(values.name);
    if (name.length < 3 || name.endsWith('-'))
      throw new Error('Use a deployment name of 3–63 characters that ends with a letter or digit');
    const namespace = identifier(values.namespace);
    const repo = await resolveRepository(io, text(values.repo));
    const origin = new URL(text(values.origin));
    if (
      origin.protocol !== 'https:' ||
      origin.username ||
      origin.password ||
      origin.pathname !== '/' ||
      origin.search ||
      origin.hash ||
      origin.port
    )
      throw new Error('Use an HTTPS origin without a path');
    if (
      origin.hostname.endsWith('.workers.dev') &&
      (!origin.hostname.startsWith(`${name}.`) || origin.hostname.split('.').length !== 4)
    )
      throw new Error('Use the deployment name and account subdomain in the workers.dev origin');
    const profile = values.profile ?? 'free';
    if (profile !== 'free' && profile !== 'paid') throw new Error('Use free or paid');
    const databases = await io.api(`/d1/database?name=${encodeURIComponent(name)}&per_page=100`);
    if (!Array.isArray(databases)) throw new Error('Invalid D1 list');
    let db = databases.map(object).find((item) => item['name'] === name);
    if (!db) {
      await io.wrangler(['d1', 'create', name, '--no-update-config']);
      const created = await io.api(`/d1/database?name=${encodeURIComponent(name)}&per_page=100`);
      if (!Array.isArray(created)) throw new Error('Invalid D1 list');
      db = created.map(object).find((item) => item['name'] === name);
    }
    if (!db) throw new Error('D1 creation did not return the database');
    try {
      const bucket = object(await io.api(`/r2/buckets/${name}`));
      if (bucket['storage_class'] && bucket['storage_class'] !== 'Standard')
        throw new Error('Use an R2 Standard bucket');
    } catch (error) {
      if (!(error instanceof ApiError) || error.status !== 404) throw error;
      await io.wrangler([
        'r2',
        'bucket',
        'create',
        name,
        '--storage-class',
        'Standard',
        '--no-update-config',
      ]);
    }
    // Refuse to adopt a bucket with public custom domains, then disable r2.dev.
    const domains = object(await io.api(`/r2/buckets/${name}/domains/custom`));
    if (!Array.isArray(domains['domains']) || domains['domains'].length)
      throw new Error('Remove R2 public custom domains before setup');
    await io.api(`/r2/buckets/${name}/domains/managed`, 'PUT', { enabled: false });
    config = await readTemplate();
    config.name = name;
    config.d1_databases[0] = {
      binding: 'INDEX',
      database_name: name,
      database_id: text(db['uuid']),
      migrations_dir: 'migrations',
    };
    config.r2_buckets[0] = { binding: 'ARTIFACTS', bucket_name: name };
    config.workers_dev = origin.hostname.endsWith('.workers.dev');
    if (!config.workers_dev) config.routes = [{ pattern: origin.hostname, custom_domain: true }];
    config.vars['GC_BATCH_SIZE'] = profile === 'free' ? '16' : '256';
    await io.writeConfig(config);
    await io.wrangler(['d1', 'migrations', 'apply', 'INDEX', '--remote', '--config', configPath]);
    const prior = await query(io, config, 'SELECT repository_id FROM scopes WHERE scope_id = ?', [
      namespace,
    ]);
    if (prior.length && prior[0]!['repository_id'] !== repo.id)
      throw new Error('Use a new namespace for a different repository');
    // Repeated setup only creates a missing policy; it never re-enables a withdrawn scope.
    await query(
      io,
      config,
      `INSERT INTO scopes (scope_id, endpoint, repository, repository_id, repository_owner_id, branch, retention_seconds, byte_limit, entry_limit, association_limit)
      VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(scope_id) DO NOTHING`,
      [
        namespace,
        `${origin.origin}/projects/${namespace}`,
        repo.name,
        repo.id,
        repo.owner,
        repo.branch,
        positive(values['retention-days'], 7) * 86400,
        positive(values['byte-limit'], 8000000000),
        positive(values['entry-limit'], 20000),
        positive(values['association-limit'], 20000),
      ],
    );
    await query(
      io,
      config,
      `UPDATE deployment SET byte_limit = coalesce(?, byte_limit),
      entry_limit = coalesce(?, entry_limit), association_limit = coalesce(?, association_limit)`,
      [
        values['byte-limit'] ? positive(values['byte-limit'], 1) : null,
        values['entry-limit'] ? positive(values['entry-limit'], 1) : null,
        values['association-limit'] ? positive(values['association-limit'], 1) : null,
      ],
    );
    const scopes = await query(io, config, 'SELECT scope_id, endpoint FROM scopes');
    if (scopes.some((row) => new URL(text(row['endpoint'])).origin !== origin.origin))
      throw new Error('Use a separate deployment for a different public origin');
    config.vars['NAMESPACES'] = JSON.stringify(scopes.map((row) => text(row['scope_id'])));
    await io.writeConfig(config);
    await updateLifecycle(io, config);
    await io.wrangler(['deploy', '--config', configPath]);
    io.print(text(scopes.find((row) => row['scope_id'] === namespace)?.['endpoint']));
    return;
  }
  config = await io.readConfig();
  if (command === 'upgrade') {
    await io.wrangler(['d1', 'migrations', 'apply', 'INDEX', '--remote', '--config', configPath]);
    await updateLifecycle(io, config);
    await io.wrangler(['deploy', '--config', configPath]);
    return;
  }
  if (command === 'status') {
    const deployment = await query(io, config, 'SELECT * FROM deployment');
    const scopes = await query(io, config, 'SELECT * FROM scopes');
    const generations = await query(
      io,
      config,
      `SELECT state, count(*) AS count, sum(charged_bytes) AS bytes,
      min(gc_after) AS earliest_cleanup FROM generations GROUP BY state`,
    );
    const db = object(await io.api(`/d1/database/${config.d1_databases[0]!.database_id}`));
    io.print(
      JSON.stringify(
        {
          deployment,
          scopes,
          generations,
          database_bytes: db['file_size'],
          warning:
            Number(db['file_size']) >= 400000000 ? 'D1 storage is at or above 400 MB' : undefined,
        },
        null,
        2,
      ),
    );
    return;
  }
  if (command === 'deployment') {
    await query(
      io,
      config,
      `UPDATE deployment SET enabled = coalesce(?, enabled), writes_enabled = coalesce(?, writes_enabled),
      byte_limit = coalesce(?, byte_limit), entry_limit = coalesce(?, entry_limit), association_limit = coalesce(?, association_limit)`,
      [
        toggle(values.enabled),
        toggle(values.writes),
        values['byte-limit'] ? positive(values['byte-limit'], 1) : null,
        values['entry-limit'] ? positive(values['entry-limit'], 1) : null,
        values['association-limit'] ? positive(values['association-limit'], 1) : null,
      ],
    );
    return;
  }
  if (command === 'teardown') {
    if (values.confirm !== config.name)
      throw new Error(
        'Pass --confirm with the exact deployment name to delete its data and resources',
      );
    const database = config.d1_databases[0]!.database_id;
    const bucket = config.r2_buckets[0]!.bucket_name;
    const databaseExists = await exists(io, `/d1/database/${database}`);
    if (databaseExists) {
      await query(io, config, 'UPDATE deployment SET enabled = 0, writes_enabled = 0');
      await query(
        io,
        config,
        `UPDATE generations SET lease_until = min(lease_until, unixepoch()),
        gc_after = min(gc_after, unixepoch() + 600), expires_at = 0`,
      );
      const remaining = await query(io, config, 'SELECT count(*) AS count FROM generations');
      if (Number(remaining[0]?.['count']) > 0)
        throw new Error(
          'Data is withdrawn. Cron will delete it after the upload grace period. Run status, then repeat teardown when generations reach zero.',
        );
    }
    // R2 rejects deletion of a nonempty bucket. Keep D1 and Cron until this succeeds.
    if (await exists(io, `/r2/buckets/${bucket}`))
      await io.wrangler(['r2', 'bucket', 'delete', bucket]);
    if (databaseExists) await io.wrangler(['d1', 'delete', database, '--skip-confirmation']);
    await io.wrangler(['delete', '--config', configPath, '--force']);
    io.print('Removed cache storage and Worker.');
    return;
  }
  const namespace = identifier(values.namespace);
  const existing = await query(io, config, 'SELECT * FROM scopes WHERE scope_id = ?', [namespace]);
  if (command === 'bind') {
    const repo = await resolveRepository(io, text(values.repo));
    if (existing.length) {
      if (existing[0]!['repository_id'] !== repo.id)
        throw new Error('Use a new namespace for a different repository');
      await query(
        io,
        config,
        `UPDATE scopes SET repository = ?, repository_owner_id = ?, branch = ?, policy_version = policy_version + 1 WHERE scope_id = ?`,
        [repo.name, repo.owner, repo.branch, namespace],
      );
    } else {
      const scopes = await query(io, config, 'SELECT endpoint FROM scopes LIMIT 1');
      const origin = new URL(text(scopes[0]?.['endpoint'])).origin;
      await query(
        io,
        config,
        `INSERT INTO scopes (scope_id, endpoint, repository, repository_id, repository_owner_id, branch) VALUES (?, ?, ?, ?, ?, ?)`,
        [namespace, `${origin}/projects/${namespace}`, repo.name, repo.id, repo.owner, repo.branch],
      );
    }
    // Retry deployment even if an earlier attempt committed the binding first.
    const all = await query(io, config, 'SELECT scope_id FROM scopes');
    config.vars['NAMESPACES'] = JSON.stringify(all.map((row) => text(row['scope_id'])));
    await io.writeConfig(config);
    await io.wrangler(['deploy', '--config', configPath]);
    io.print(
      text(
        (
          await query(io, config, 'SELECT endpoint FROM scopes WHERE scope_id = ?', [namespace])
        )[0]?.['endpoint'],
      ),
    );
    return;
  }
  if (!existing.length) throw new Error('Namespace does not exist');
  if (command === 'purge') {
    if (values.confirm !== namespace)
      throw new Error(
        'Pass --confirm with the exact namespace to withdraw and delete its public data',
      );
    await query(
      io,
      config,
      'UPDATE scopes SET enabled = 0, writes_enabled = 0, policy_version = policy_version + 1 WHERE scope_id = ?',
      [namespace],
    );
    await query(
      io,
      config,
      `UPDATE generations SET expires_at = 0, lease_until = min(lease_until, unixepoch()),
      gc_after = min(gc_after, unixepoch() + 600) WHERE scope_id = ?`,
      [namespace],
    );
    io.print('Namespace withdrawn. Cron will remove its data after the upload grace period.');
    return;
  }
  // Install a longer lifecycle before publishing a longer retention policy.
  if (values['retention-days']) {
    const days = positive(values['retention-days'], 7);
    const max = await query(
      io,
      config,
      'SELECT retention_high_water_seconds AS retention FROM deployment',
    );
    await io.lifecycle(
      config.r2_buckets[0]!.bucket_name,
      lifecycleRules(Math.max(days, Math.ceil(Number(max[0]?.['retention']) / 86400))),
    );
  }
  await query(
    io,
    config,
    `UPDATE scopes SET enabled = coalesce(?, enabled), writes_enabled = coalesce(?, writes_enabled),
    retention_seconds = coalesce(?, retention_seconds), byte_limit = coalesce(?, byte_limit), entry_limit = coalesce(?, entry_limit),
    association_limit = coalesce(?, association_limit), policy_version = policy_version + 1 WHERE scope_id = ?`,
    [
      toggle(values.enabled),
      toggle(values.writes),
      values['retention-days'] ? positive(values['retention-days'], 7) * 86400 : null,
      values['byte-limit'] ? positive(values['byte-limit'], 1) : null,
      values['entry-limit'] ? positive(values['entry-limit'], 1) : null,
      values['association-limit'] ? positive(values['association-limit'], 1) : null,
      namespace,
    ],
  );
}

export class ApiError extends Error {
  constructor(public status: number) {
    super(`Cloudflare API failed (HTTP ${status})`);
  }
}

async function jsonResponse(response: Response): Promise<unknown> {
  if (!response.ok) {
    void response.body?.cancel();
    throw new ApiError(response.status);
  }
  const reader = response.body!.getReader();
  let size = 0;
  const chunks: Uint8Array[] = [];
  try {
    while (true) {
      const part = await reader.read();
      if (part.done) break;
      size += part.value.length;
      if (size > 4 * 1024 * 1024) throw new Error('API response too large');
      chunks.push(part.value);
    }
  } finally {
    await reader.cancel();
  }
  return JSON.parse(Buffer.concat(chunks).toString('utf8'));
}

export const operatorIO: OperatorIO = {
  async api(path, method = 'GET', body) {
    const account = process.env['CLOUDFLARE_ACCOUNT_ID'];
    const token = process.env['CLOUDFLARE_API_TOKEN'];
    if (!account || !/^[a-f0-9]{32}$/.test(account) || !token)
      throw new Error(
        'Set CLOUDFLARE_ACCOUNT_ID and CLOUDFLARE_API_TOKEN in the operator environment',
      );
    const response = await fetch(
      `https://api.cloudflare.com/client/v4/accounts/${account}${path}`,
      {
        method,
        headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }),
        signal: AbortSignal.timeout(30_000),
        redirect: 'error',
      },
    );
    const result = object(await jsonResponse(response));
    if (result['success'] !== true) throw new Error('Cloudflare API operation failed');
    return result['result'];
  },
  async github(repository) {
    return jsonResponse(
      await fetch(`https://api.github.com/repos/${repository}`, {
        headers: {
          Accept: 'application/vnd.github+json',
          'X-GitHub-Api-Version': '2022-11-28',
          'User-Agent': 'vp-remote-cache-operator',
        },
        redirect: 'error',
        signal: AbortSignal.timeout(15_000),
      }),
    );
  },
  async wrangler(args) {
    const wrangler = join(dirname(require.resolve('wrangler/package.json')), 'bin/wrangler.js');
    await new Promise<void>((resolve, reject) => {
      const child = spawn(process.execPath, [wrangler, ...args], {
        cwd: root,
        stdio: 'inherit',
        shell: false,
      });
      child.once('error', reject);
      child.once('exit', (code) =>
        code === 0 ? resolve() : reject(new Error(`Wrangler failed (exit ${code})`)),
      );
    });
  },
  async readConfig() {
    return JSON.parse(await readFile(configPath, 'utf8'));
  },
  async writeConfig(config) {
    await writeFile(configPath, JSON.stringify(config, null, 2) + '\n');
  },
  async lifecycle(bucket, rules) {
    const dir = await mkdtemp(join(tmpdir(), 'vp-cache-lifecycle-'));
    try {
      const path = join(dir, 'lifecycle.json');
      await writeFile(path, JSON.stringify(rules));
      await operatorIO.wrangler([
        'r2',
        'bucket',
        'lifecycle',
        'set',
        bucket,
        '--file',
        path,
        '--force',
      ]);
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  },
  print: (message) => console.log(message),
};

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  runOperator(process.argv.slice(2), operatorIO).catch((error) => {
    console.error(error instanceof Error ? error.message : 'Operator command failed');
    process.exitCode = 1;
  });
}
