import { randomUUID } from 'node:crypto';
import { setTimeout } from 'node:timers/promises';
import { pathToFileURL } from 'node:url';
import { encode } from 'cborg';
import {
  operatorIO,
  query,
  readTemplate,
  runOperator,
  type Config,
  type OperatorIO,
} from './operator.ts';

// These checks use no upload credentials and do not publish cache data.
export async function checkDeployment(
  endpoint: string,
  deployment: string,
  enabled: boolean,
  request: typeof fetch = fetch,
): Promise<void> {
  const key = new TextEncoder().encode(`deployment-check:${randomUUID()}`);
  const checks = [
    { path: 'store', status: enabled ? 401 : 404, body: undefined },
    { path: 'fetch', status: enabled ? 400 : 404, body: new Uint8Array() },
    { path: 'fetch', status: 404, body: encode({ key, secondary_key: key }) },
  ];
  for (const check of checks) {
    const response = await request(`${endpoint}/${check.path}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/cbor' },
      body: check.body,
      redirect: 'error',
      signal: AbortSignal.timeout(10_000),
    });
    await response.body?.cancel();
    if (
      response.status !== check.status ||
      response.headers.get('X-Remote-Cache-Deployment') !== deployment ||
      response.headers.get('Cache-Control') !== 'no-store'
    )
      throw new Error(
        `Deployment check failed for /${check.path}: expected ${check.status}, received ${response.status}; check the deployed revision and cache headers`,
      );
  }
}

export async function deploy(
  config: Config,
  io: OperatorIO = operatorIO,
  env: Record<string, string | undefined> = process.env,
  request: typeof fetch = fetch,
  wait: (ms: number) => Promise<unknown> = setTimeout,
): Promise<void> {
  const repository = env['CACHE_REPOSITORY'] ?? config.vars['CACHE_REPOSITORY'];
  if (!repository || repository === 'owner/repository')
    throw new Error(
      'Set CACHE_REPOSITORY to your public GitHub owner/repository in the deployment settings',
    );
  const namespace = env['CACHE_NAMESPACE'] ?? config.vars['CACHE_NAMESPACE'] ?? 'cache';
  const profile = env['CACHE_PROFILE'] ?? config.vars['CACHE_PROFILE'] ?? 'free';
  const database = config.d1_databases.find((binding) => binding.binding === 'INDEX');
  const bucket = config.r2_buckets.find((binding) => binding.binding === 'ARTIFACTS');
  if (
    !database ||
    !bucket ||
    !/^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/i.test(database.database_id) ||
    database.database_id === '00000000-0000-0000-0000-000000000000' ||
    !/^[a-z0-9][a-z0-9-]{1,61}[a-z0-9]$/.test(bucket.bucket_name)
  )
    throw new Error(
      'Use the D1 INDEX and R2 ARTIFACTS bindings provisioned by Deploy to Cloudflare',
    );
  if (env['WRANGLER_CI_OVERRIDE_NAME'] && env['WRANGLER_CI_OVERRIDE_NAME'] !== config.name)
    throw new Error(
      'The Worker name in wrangler.jsonc must match the connected Workers Builds project',
    );
  const account = (await io.api('/workers/subdomain')) as { subdomain?: unknown };
  if (typeof account.subdomain !== 'string' || !/^[a-z0-9][a-z0-9-]{0,62}$/.test(account.subdomain))
    throw new Error('Create a workers.dev subdomain in your Cloudflare account before deploying');
  const origin = `https://${config.name}.${account.subdomain}.workers.dev`;
  const deployment = env['WORKERS_CI_BUILD_UUID'] || randomUUID();
  await runOperator(
    [
      'setup',
      '--name',
      config.name,
      '--namespace',
      namespace,
      '--repo',
      repository,
      '--origin',
      origin,
      '--profile',
      profile,
    ],
    {
      ...io,
      async writeConfig(value) {
        value.vars['DEPLOYMENT_ID'] = deployment;
        await io.writeConfig(value);
      },
    },
    { database, bucket },
  );
  const deployed = await io.readConfig();
  const policies = await query(
    io,
    deployed,
    'SELECT scopes.enabled AS scope_enabled, deployment.enabled AS deployment_enabled FROM scopes CROSS JOIN deployment WHERE scope_id = ?',
    [namespace],
  );
  if (policies.length !== 1) throw new Error('The deployed namespace has no policy');
  const enabled = policies[0]!['scope_enabled'] === 1 && policies[0]!['deployment_enabled'] === 1;
  const endpoint = `${origin}/projects/${namespace}`;
  // New workers.dev routes and revisions can take a short time to reach the edge.
  for (let attempt = 0; ; attempt++) {
    try {
      await checkDeployment(endpoint, deployment, enabled, request);
      break;
    } catch (error) {
      if (attempt === 9) throw error;
      await wait(3000);
    }
  }
  io.print(`Deployment checks passed. Cache endpoint: ${endpoint}`);
  if (!enabled)
    io.print('This namespace remains disabled. Deployment did not change its access policy.');
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  readTemplate()
    .then((config) => deploy(config))
    .catch((error) => {
      console.error(error instanceof Error ? error.message : 'Deployment failed');
      process.exitCode = 1;
    });
}
