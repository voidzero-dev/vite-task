import assert from 'node:assert/strict';
import { createHash, randomUUID } from 'node:crypto';
import { setTimeout as delay } from 'node:timers/promises';
import { encode, decode } from 'cborg';
import { defaults, MiB } from '../../src/limits.ts';
import { bytes, seed, sqlBytes, type Admin, type Fixture } from './fixtures.ts';

export interface Result {
  name: string;
  status: 'passed' | 'failed';
  duration_ms: number;
  error?: string;
}
export interface Report {
  deployment: string;
  endpoint: string;
  mode: 'push' | 'read-only';
  started_at: string;
  results: Result[];
}
export interface Options {
  origin: string;
  deployment: string;
  admin: Admin;
  request: (url: string, init?: RequestInit) => Promise<Response>;
  token: (audience: string) => Promise<string>;
  writes: boolean;
  full: boolean;
  cron: boolean;
  cronTimeoutMs?: number;
  pollMs?: number;
  record?: (report: Report) => Promise<void>;
}

const digest = (value: Uint8Array) => createHash('sha256').update(value).digest('hex');
function blobId(result: Record<string, unknown>): string {
  assert.equal(typeof result['blob_id'], 'string');
  return result['blob_id'] as string;
}

export async function runSuite(options: Options): Promise<Report> {
  const { origin, admin, deployment } = options;
  const report: Report = {
    deployment,
    endpoint: `${origin}/projects/e2e`,
    mode: options.writes ? 'push' : 'read-only',
    started_at: new Date().toISOString(),
    results: [],
  };
  const prefix = randomUUID();
  async function check(name: string, action: () => Promise<void>) {
    const start = Date.now();
    try {
      await action();
      report.results.push({ name, status: 'passed', duration_ms: Date.now() - start });
    } catch (error) {
      // Reports contain test names, never request headers, tokens, or server bodies.
      report.results.push({
        name,
        status: 'failed',
        duration_ms: Date.now() - start,
        error:
          error instanceof assert.AssertionError
            ? 'Assertion failed'
            : 'Request or administration failed',
      });
      await options.record?.(report);
      throw new Error(`Cloudflare e2e failed: ${name}`, { cause: error });
    }
    await options.record?.(report);
  }
  async function call(scope: string, path: string, status: number, init: RequestInit = {}) {
    const response = await options.request(`${origin}/projects/${scope}/${path}`, {
      ...init,
      redirect: 'error',
      signal: AbortSignal.timeout(path === 'store' ? 130000 : 30000),
    });
    assert.equal(response.status, status, `${scope}/${path}: unexpected HTTP status`);
    assert.equal(
      response.headers.get('X-Remote-Cache-Deployment'),
      deployment,
      'Deployment changed during verification',
    );
    assert.equal(response.headers.get('Cache-Control'), 'no-store');
    assert.ok(response.headers.get('X-Request-Id'));
    if (status === 200 && path.startsWith('blob/'))
      assert.equal(response.headers.get('Content-Type'), 'application/octet-stream');
    if (status >= 400)
      assert.equal(response.headers.get('Content-Type'), 'text/plain; charset=utf-8');
    return response;
  }
  async function body(response: Response): Promise<Uint8Array> {
    const reader = response.body?.getReader();
    if (!reader) return new Uint8Array();
    const chunks: Uint8Array[] = [];
    let size = 0;
    try {
      while (true) {
        const part = await reader.read();
        if (part.done) break;
        size += part.value.length;
        assert.ok(size <= defaults.blob + MiB, 'Response exceeds the test limit');
        chunks.push(part.value);
      }
      const combined = Buffer.concat(chunks);
      return new Uint8Array(combined.buffer, combined.byteOffset, combined.byteLength);
    } finally {
      await reader.cancel();
    }
  }
  async function envelope(response: Response): Promise<Record<string, unknown>> {
    assert.equal(response.headers.get('Content-Type'), 'application/cbor');
    const value: unknown = decode(await body(response));
    assert.ok(value && typeof value === 'object' && !Array.isArray(value));
    return value as Record<string, unknown>;
  }
  async function lookup(key: Uint8Array, secondary: Uint8Array, status = 200, scope = 'e2e') {
    return call(scope, 'fetch', status, {
      method: 'POST',
      headers: { 'Content-Type': 'application/cbor' },
      body: encode({ key, secondary_key: secondary }),
    });
  }
  async function store(
    key: Uint8Array,
    secondary: Uint8Array,
    value: Uint8Array,
    blob?: Uint8Array,
    options: { blobFirst?: boolean; status?: number; token?: string; stream?: boolean } = {},
  ) {
    const form = new FormData();
    const addBlob = () => {
      if (blob !== undefined)
        form.append('blob', new Blob([blob], { type: 'application/octet-stream' }));
    };
    if (options.blobFirst) addBlob();
    form.append(
      'metadata',
      new Blob([encode({ key, secondary_key: secondary, value })], { type: 'application/cbor' }),
    );
    if (!options.blobFirst) addBlob();
    const authorization = `Bearer ${options.token ?? (await optionsToken())}`;
    if (!options.stream)
      return call('e2e', 'store', options.status ?? 200, {
        method: 'POST',
        headers: { Authorization: authorization },
        body: form,
      });
    const request = new Request(`${origin}/projects/e2e/store`, { method: 'POST', body: form });
    return call('e2e', 'store', options.status ?? 200, {
      method: 'POST',
      headers: { ...Object.fromEntries(request.headers), Authorization: authorization },
      body: request.body,
      duplex: 'half',
    } as RequestInit);
  }
  const optionsToken = () => options.token(`${origin}/projects/e2e`);
  let fixture: Fixture;
  await check('deployed revision and D1/R2 readiness', async () => {
    fixture = await seed(
      admin,
      'e2e',
      `${prefix}-fixture`,
      bytes(`value:${deployment}`),
      bytes(`blob:${deployment}`),
    );
    const until = Date.now() + 120000;
    while (true) {
      const response = await options.request(`${origin}/projects/e2e/fetch`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/cbor' },
        body: encode({ key: fixture.key, secondary_key: fixture.secondary }),
        redirect: 'error',
        signal: AbortSignal.timeout(15000),
      });
      if (
        response.status === 200 &&
        response.headers.get('X-Remote-Cache-Deployment') === deployment
      ) {
        assert.deepEqual((await envelope(response)).value, fixture.value);
        break;
      }
      await response.body?.cancel();
      assert.ok(Date.now() < until, 'Deployment did not become ready');
      await delay(options.pollMs ?? 3000);
    }
  });
  const f = fixture!;
  await check('manual verification fixture is available at the advertised endpoint', async () => {
    const manual = await envelope(
      await lookup(
        bytes('manual-verification'),
        bytes('manual-verification-secondary'),
        200,
        'manual',
      ),
    );
    assert.equal(manual['kind'], 'exact');
    assert.deepEqual(manual['value'], bytes(`deployment:${deployment}`));
    assert.deepEqual(
      await body(await call('manual', `blob/${blobId(manual)}`, 200)),
      bytes('Public remote cache verification blob\n'),
    );
  });
  await check('anonymous exact/fallback reads preserve data and counters', async () => {
    // Real Cron can delete older runs during these reads. Compare invariants and
    // this live fixture, rather than assuming global totals cannot decrease.
    const accounting = `SELECT
      charged_bytes - (SELECT coalesce(sum(charged_bytes), 0) FROM generations) AS bytes_delta,
      entry_count - (SELECT count(*) FROM entries) AS entries_delta,
      association_count - (SELECT count(*) FROM associations) AS associations_delta FROM deployment`;
    const before = await admin.sql(
      'SELECT charged_bytes, expires_at, state FROM generations WHERE generation_id = ?',
      [f.generation],
    );
    assert.deepEqual(await admin.sql(accounting), [
      { bytes_delta: 0, entries_delta: 0, associations_delta: 0 },
    ]);
    assert.deepEqual(await envelope(await lookup(f.key, f.secondary)), {
      kind: 'exact',
      value: f.value,
      blob_id: f.blobId,
    });
    assert.deepEqual(await envelope(await lookup(bytes('missing'), f.secondary)), {
      kind: 'fallback',
      key: f.key,
      value: f.value,
      blob_id: f.blobId,
    });
    assert.deepEqual(await body(await call('e2e', `blob/${f.blobId}`, 200)), f.blob);
    assert.deepEqual(
      await admin.sql(
        'SELECT charged_bytes, expires_at, state FROM generations WHERE generation_id = ?',
        [f.generation],
      ),
      before,
    );
    assert.deepEqual(await admin.sql(accounting), [
      { bytes_delta: 0, entries_delta: 0, associations_delta: 0 },
    ]);
  });
  await check('missing data, namespace isolation, and malformed reads', async () => {
    await body(await lookup(bytes(`${prefix}-missing`), bytes('missing'), 404));
    await body(await lookup(f.key, f.secondary, 404, 'other'));
    await body(await call('other', `blob/${f.blobId}`, 404));
    await body(await call('unknown', 'fetch', 404, { method: 'POST' }));
    await body(
      await call('e2e', 'fetch', 400, {
        method: 'POST',
        headers: { 'Content-Type': 'application/cbor' },
        body: Uint8Array.of(255),
      }),
    );
    await body(await lookup(new Uint8Array(defaults.key + 1), bytes('S'), 413));
  });
  await check('missing, forged, and wrong-audience credentials cannot write', async () => {
    const before = new Set(
      (await admin.sql('SELECT generation_id FROM generations')).map((row) => row['generation_id']),
    );
    await body(await call('e2e', 'store', 401, { method: 'POST' }));
    await body(
      await call('e2e', 'store', 401, {
        method: 'POST',
        headers: { Authorization: 'Bearer forged' },
      }),
    );
    await body(
      await store(f.key, f.secondary, bytes('rejected'), undefined, {
        token: await options.token(`${origin}/projects/other`),
        status: 403,
      }),
    );
    if (!options.writes)
      await body(await store(f.key, f.secondary, bytes('rejected'), undefined, { status: 403 }));
    assert.ok(
      (await admin.sql('SELECT generation_id FROM generations')).every((row) =>
        before.has(row['generation_id']),
      ),
    );
  });
  await check('policy withdrawal takes effect without redeployment', async () => {
    try {
      await admin.sql("UPDATE scopes SET enabled = 0 WHERE scope_id = 'e2e'");
      await body(await lookup(f.key, f.secondary, 404));
      await body(await call('e2e', `blob/${f.blobId}`, 404));
    } finally {
      await admin.sql("UPDATE scopes SET enabled = 1 WHERE scope_id = 'e2e'");
    }
    assert.deepEqual((await envelope(await lookup(f.key, f.secondary))).value, f.value);
  });
  await check('missing live R2 objects have the correct errors', async () => {
    try {
      await admin.delete(f.valueObject);
      await body(await lookup(f.key, f.secondary, 503));
    } finally {
      await admin.put(f.valueObject, f.value);
    }
    try {
      await admin.delete(f.blobObject);
      await body(await call('e2e', `blob/${f.blobId}`, 404));
    } finally {
      await admin.put(f.blobObject, f.blob!);
    }
  });

  if (options.writes) {
    await check('real GitHub OIDC permits opaque and empty HTTP stores', async () => {
      const key = new Uint8Array(),
        secondary = Uint8Array.of(0, 255);
      const value = Uint8Array.of(0, 255, 159, 255);
      assert.deepEqual(await envelope(await store(key, secondary, value)), { blob_id: null });
      const empty = await envelope(
        await store(key, secondary, value, new Uint8Array(), { blobFirst: true, stream: true }),
      );
      assert.equal(typeof empty.blob_id, 'string');
      assert.equal((await body(await call('e2e', `blob/${blobId(empty)}`, 200))).length, 0);
      assert.deepEqual((await envelope(await lookup(key, secondary))).value, value);
    });
    await check('secondary reassignment and replacement keep both mappings coherent', async () => {
      const a = bytes(`${prefix}-A`),
        b = bytes(`${prefix}-B`),
        s = bytes(`${prefix}-S`),
        t = bytes(`${prefix}-T`);
      const old = await envelope(await store(a, s, bytes('old'), bytes('old-blob')));
      await body(await store(b, s, bytes('B')));
      assert.deepEqual((await envelope(await lookup(a, s))).value, bytes('old'));
      assert.deepEqual((await envelope(await lookup(bytes('missing'), s))).key, b);
      await body(await store(a, t, bytes('replacement')));
      assert.deepEqual(
        (await envelope(await lookup(bytes('missing'), t))).value,
        bytes('replacement'),
      );
      assert.deepEqual(
        await body(await call('e2e', `blob/${blobId(old)}`, 200)),
        bytes('old-blob'),
      );
    });
    await check('R2 multipart upload and download preserve bytes', async () => {
      const value = new Uint8Array(250000).fill(149),
        blob = new Uint8Array(5 * MiB + 17).fill(61);
      const key = bytes(`${prefix}-large`);
      const stored = await envelope(await store(key, key, value, blob, { blobFirst: true }));
      assert.deepEqual((await envelope(await lookup(key, key))).value, value);
      assert.equal(
        digest(await body(await call('e2e', `blob/${blobId(stored)}`, 200))),
        digest(blob),
      );
    });
    await check('concurrent HTTP stores select one complete generation', async () => {
      const key = bytes(`${prefix}-concurrent`);
      await Promise.all(
        [0, 1].map(async (i) => body(await store(key, key, bytes(String(i)), bytes(String(i))))),
      );
      const selected = await envelope(await lookup(key, key));
      assert.deepEqual(
        await body(await call('e2e', `blob/${blobId(selected)}`, 200)),
        selected.value,
      );
    });
    await check('malformed uploads and exhausted quotas preserve published data', async () => {
      const token = await optionsToken();
      await body(
        await call('e2e', 'store', 400, {
          method: 'POST',
          headers: {
            Authorization: `Bearer ${token}`,
            'Content-Type': 'multipart/form-data; boundary=x',
          },
          body: '--x\r\nContent-Disposition: form-data; name="metadata"\r\nContent-Type: application/cbor\r\n\r\ntruncated',
        }),
      );
      const limit = (await admin.sql("SELECT byte_limit FROM scopes WHERE scope_id = 'e2e'"))[0]![
        'byte_limit'
      ];
      assert.equal(typeof limit, 'number');
      try {
        await admin.sql(
          "UPDATE scopes SET byte_limit = max(1, charged_bytes) WHERE scope_id = 'e2e'",
        );
        await body(await store(f.key, f.secondary, bytes('rejected'), undefined, { status: 503 }));
      } finally {
        await admin.sql("UPDATE scopes SET byte_limit = ? WHERE scope_id = 'e2e'", [Number(limit)]);
      }
      assert.deepEqual((await envelope(await lookup(f.key, f.secondary))).value, f.value);
    });
    if (options.full)
      await check('maximum values and concurrent 64 MiB HTTP uploads', async () => {
        await Promise.all(
          [0, 1].map(async (i) => {
            const key = bytes(`${prefix}-maximum-${i}`),
              value = new Uint8Array(defaults.value).fill(17 + i);
            const blob = new Uint8Array(defaults.blob).fill(29 + i);
            const stored = await envelope(
              await store(key, key, value, blob, { blobFirst: i === 0 }),
            );
            assert.equal(
              digest((await envelope(await lookup(key, key))).value as Uint8Array),
              digest(value),
            );
            assert.equal(
              digest(await body(await call('e2e', `blob/${blobId(stored)}`, 200))),
              digest(blob),
            );
          }),
        );
      });
  }

  await check(
    options.cron
      ? 'real Cron deletes expired objects and releases accounting'
      : 'expired generations are immediately unavailable',
    async () => {
      const expired = await seed(
        admin,
        'e2e',
        `${prefix}-expired`,
        bytes('expired'),
        bytes('expired-blob'),
      );
      await admin.sql(
        'UPDATE generations SET expires_at = 0, gc_after = 0 WHERE generation_id = ?',
        [expired.generation],
      );
      await body(await lookup(expired.key, expired.secondary, 404));
      await body(await call('e2e', `blob/${expired.blobId}`, 404));
      if (options.cron) {
        const until = Date.now() + (options.cronTimeoutMs ?? 25 * 60000);
        while (
          (
            await admin.sql('SELECT generation_id FROM generations WHERE generation_id = ?', [
              expired.generation,
            ])
          ).length
        ) {
          assert.ok(Date.now() < until, 'Cron did not remove the generation');
          await delay(options.pollMs ?? 30000);
        }
        assert.equal(await admin.exists(expired.valueObject), false);
        assert.equal(await admin.exists(expired.blobObject), false);
        assert.equal(
          (
            await admin.sql(
              `SELECT count(*) AS count FROM entries WHERE scope_id = 'e2e' AND key = ${sqlBytes(expired.key)}`,
            )
          )[0]!['count'],
          0,
        );
        const accounting = (
          await admin.sql(
            'SELECT charged_bytes, (SELECT coalesce(sum(charged_bytes), 0) FROM generations) AS expected FROM deployment',
          )
        )[0]!;
        assert.equal(accounting['charged_bytes'], accounting['expected']);
      }
    },
  );
  return report;
}
