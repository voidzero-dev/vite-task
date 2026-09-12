import assert from 'node:assert/strict';
import { test } from 'node:test';
import { encode } from 'cborg';
import { bytes, decodeResponse, endpoint, harness } from './helpers.ts';
import { getScope, publish, reserve } from '../src/database.ts';

void test('Workers D1/R2: exact/fallback, replacement, empty blobs, namespace isolation, and read-only access', async () => {
  const h = await harness();
  try {
    const a = Uint8Array.of(0, 255),
      b = bytes('B'),
      c = bytes('C'),
      s = bytes('S'),
      t = new Uint8Array();
    assert.equal((await h.fetch(a, s)).status, 404);
    const first = await h.store(a, s, bytes('VA'), bytes('old'));
    assert.equal(first.status, 200, await first.clone().text());
    const firstBlob = (await decodeResponse(first)).blob_id;
    assert.equal((await h.store(b, s, bytes('VB'))).status, 200);
    assert.deepEqual(await decodeResponse(await h.fetch(a, s)), {
      kind: 'exact',
      value: bytes('VA'),
      blob_id: firstBlob,
    });
    assert.deepEqual(await decodeResponse(await h.fetch(c, s)), {
      kind: 'fallback',
      key: b,
      value: bytes('VB'),
      blob_id: null,
    });
    const replacement = await h.store(a, t, bytes('VA2'), new Uint8Array(), { blobFirst: true });
    assert.equal(replacement.status, 200);
    const emptyBlob = (await decodeResponse(replacement)).blob_id;
    assert.equal(typeof emptyBlob, 'string');
    assert.equal(await (await h.mf.dispatchFetch(`${endpoint}/blob/${firstBlob}`)).text(), 'old');
    assert.equal((await h.mf.dispatchFetch(`${endpoint}/blob/${emptyBlob}`)).status, 200);
    assert.equal(
      (await h.mf.dispatchFetch(`${endpoint}/blob/${emptyBlob}`)).headers.get('Content-Length'),
      '0',
    );
    assert.equal((await h.fetch(a, s, 'other')).status, 404);
    assert.equal(
      (await h.mf.dispatchFetch(`https://cache.example.com/projects/other/blob/${firstBlob}`))
        .status,
      404,
    );
    assert.equal((await h.store(a, s, bytes('other'), undefined, { scope: 'other' })).status, 403);
    assert.equal(
      (
        await h.store(a, s, bytes('other'), undefined, {
          scope: 'other',
          token: await h.token({ aud: endpoint.replace('/test', '/other') }),
        })
      ).status,
      200,
    );
    assert.deepEqual((await decodeResponse(await h.fetch(a, s, 'other'))).value, bytes('other'));
    assert.deepEqual((await decodeResponse(await h.fetch(a, s))).value, bytes('VA2'));
    const before = await h.db.prepare('SELECT * FROM deployment').first();
    await h.fetch(a, t);
    await h.fetch(c, t);
    assert.deepEqual(await h.db.prepare('SELECT * FROM deployment').first(), before);
    await h.db.prepare("UPDATE scopes SET enabled = 0 WHERE scope_id = 'test'").run();
    assert.equal((await h.fetch(a, t)).status, 404);
    assert.equal((await h.mf.dispatchFetch(`${endpoint}/blob/${firstBlob}`)).status, 404);
  } finally {
    await h.close();
  }
});

void test('signed GitHub policy denies forks, wrong owners, private repos, non-push events, branches, audiences, and invalid times', async () => {
  const h = await harness();
  try {
    for (const claims of [
      { repository_id: '999' },
      { repository_id: 123 },
      { repository_owner_id: '999' },
      { repository_visibility: 'private' },
      { event_name: 'pull_request' },
      { event_name: 'pull_request_target' },
      { event_name: 'workflow_run' },
      { ref: 'refs/heads/feature' },
      { ref_type: 'tag' },
      { aud: 'https://attacker.example' },
      { aud: [endpoint] },
    ])
      assert.equal(
        (
          await h.store(bytes('A'), bytes('S'), bytes('V'), undefined, {
            token: await h.token(claims),
          })
        ).status,
        403,
      );
    const now = Math.floor(Date.now() / 1000);
    for (const claims of [
      { exp: now - 1 },
      { nbf: now + 100 },
      { iat: now + 100 },
      { iat: 'bad' },
      { exp: null },
      { iss: 'https://attacker.example' },
    ]) {
      assert.equal(
        (
          await h.store(bytes('A'), bytes('S'), bytes('V'), undefined, {
            token: await h.token(claims),
          })
        ).status,
        401,
      );
    }
    assert.equal(
      (await h.mf.dispatchFetch(`${endpoint}/store`, { method: 'POST', body: 'not read' })).status,
      401,
    );
    assert.equal(
      (await h.store(bytes('A'), bytes('S'), bytes('V'), undefined, { token: 'forged' })).status,
      401,
    );
    const valid = await h.token();
    const segments = valid.split('.');
    const signature = Buffer.from(segments[2]!, 'base64url');
    signature[0] = signature[0]! ^ 1;
    segments[2] = signature.toString('base64url');
    assert.equal(
      (await h.store(bytes('A'), bytes('S'), bytes('V'), undefined, { token: segments.join('.') }))
        .status,
      401,
    );
    for (let i = 0; i < 5; i++)
      assert.equal(
        (
          await h.store(bytes('A'), bytes('S'), bytes('V'), undefined, {
            token: await h.token({}, `unknown-${i}`),
          })
        ).status,
        401,
      );
    assert.equal(h.jwksRequests(), 1);
    assert.equal((await h.db.prepare('SELECT count(*) AS n FROM generations').first())!.n, 0);
    assert.equal((await h.store(bytes('A'), bytes('S'), bytes('V'))).status, 200);
  } finally {
    await h.close();
  }
});

void test('JWKS outages fail closed and have a refresh cooldown', async () => {
  const h = await harness({ jwksStatus: 503 });
  try {
    for (let i = 0; i < 3; i++)
      assert.equal((await h.store(bytes('A'), bytes('S'), bytes('V'))).status, 503);
    assert.equal(h.jwksRequests(), 1);
  } finally {
    await h.close();
  }
});

void test('publication guard and quotas preserve BOTH mappings and charged bytes', async () => {
  const h = await harness();
  try {
    assert.equal((await h.store(bytes('A'), bytes('S'), bytes('old'))).status, 200);
    const scope = await getScope(h.db, 'test');
    for (const mutate of [
      (id: string) =>
        h.db
          .prepare('UPDATE generations SET lease_until = 0 WHERE generation_id = ?')
          .bind(id)
          .run(),
      (id: string) =>
        h.db.prepare('UPDATE generations SET token_exp = 0 WHERE generation_id = ?').bind(id).run(),
      () =>
        h.db
          .prepare("UPDATE scopes SET policy_version = policy_version + 1 WHERE scope_id = 'test'")
          .run(),
    ]) {
      const current = await getScope(h.db, 'test');
      const g = await reserve(h.db, current, Math.floor(Date.now() / 1000) + 300, 100);
      g.value_size = 3;
      await h.bucket.put(g.value_object, 'new');
      await mutate(g.generation_id);
      await assert.rejects(publish(h.db, g, bytes('A'), bytes('T')), { status: 503 });
      assert.equal(
        (await h.db
          .prepare('SELECT state FROM generations WHERE generation_id = ?')
          .bind(g.generation_id)
          .first())!.state,
        'uploading',
      );
      assert.deepEqual(
        (await decodeResponse(await h.fetch(bytes('A'), bytes('S')))).value,
        bytes('old'),
      );
      assert.equal((await h.fetch(bytes('missing'), bytes('T'))).status, 404);
    }
    assert.equal(scope.repository_id, '123');
    await h.db.prepare('UPDATE deployment SET association_limit = 1').run();
    assert.equal((await h.store(bytes('A'), bytes('new-S'), bytes('new'))).status, 503);
    assert.deepEqual(
      (await decodeResponse(await h.fetch(bytes('A'), bytes('S')))).value,
      bytes('old'),
    );
    assert.equal((await h.store(bytes('A'), bytes('S'), bytes('update'))).status, 200);
    await h.db.prepare('UPDATE deployment SET byte_limit = charged_bytes').run();
    assert.equal((await h.store(bytes('A'), bytes('S'), bytes('new'))).status, 503);
  } finally {
    await h.close();
  }
});

void test('large sequential R2 multipart stores and concurrent stores return coherent generations', async () => {
  const h = await harness();
  try {
    const blob = new Uint8Array(5 * 1024 * 1024 + 123).fill(37);
    const response = await h.store(bytes('large'), bytes('S'), bytes('large-value'), blob, {
      blobFirst: true,
    });
    assert.equal(response.status, 200, await response.clone().text());
    const blobId = (await decodeResponse(response)).blob_id;
    const download = await h.mf.dispatchFetch(`${endpoint}/blob/${blobId}`);
    assert.deepEqual(new Uint8Array(await download.arrayBuffer()), blob);
    const stored = await Promise.all(
      Array.from({ length: 2 }, (_, i) =>
        h.store(bytes('race'), bytes('race'), bytes(String(i)), bytes(String(i))),
      ),
    );
    for (const result of stored) assert.equal(result.status, 200);
    const final = await decodeResponse(await h.fetch(bytes('race'), bytes('race')));
    assert.deepEqual(
      new Uint8Array(
        await (await h.mf.dispatchFetch(`${endpoint}/blob/${final.blob_id}`)).arrayBuffer(),
      ),
      final.value,
    );
    const accounting = await h.db
      .prepare(
        'SELECT charged_bytes, (SELECT sum(charged_bytes) FROM generations) AS expected FROM deployment',
      )
      .first();
    assert.ok(accounting);
    assert.equal(accounting.charged_bytes, accounting.expected);
    // Cleanup also accepts a recorded upload ID whose multipart upload completed.
    await h.db
      .prepare('UPDATE generations SET expires_at = 0, gc_after = 0 WHERE blob_id = ?')
      .bind(blobId)
      .run();
    await (await h.mf.getWorker()).scheduled();
    assert.equal((await h.mf.dispatchFetch(`${endpoint}/blob/${blobId}`)).status, 404);
    assert.equal(
      (await h.db
        .prepare('SELECT count(*) AS n FROM generations WHERE blob_id = ?')
        .bind(blobId)
        .first())!.n,
      0,
    );
  } finally {
    await h.close();
  }
});

void test('retention, retirement, abandoned uploads, and bounded cleanup protect replacements', async () => {
  const h = await harness();
  try {
    const oldBlob = (
      await decodeResponse(await h.store(bytes('A'), bytes('S'), bytes('old'), bytes('old')))
    ).blob_id;
    await h.store(bytes('A'), bytes('S'), bytes('new'), bytes('new'));
    await h.db.prepare("UPDATE generations SET gc_after = 0 WHERE state = 'retired'").run();
    assert.equal((await h.mf.dispatchFetch(`${endpoint}/blob/${oldBlob}`)).status, 404);
    await (await h.mf.getWorker()).scheduled();
    assert.deepEqual(
      (await decodeResponse(await h.fetch(bytes('A'), bytes('S')))).value,
      bytes('new'),
    );
    const expiredBlob = (await decodeResponse(await h.fetch(bytes('A'), bytes('S')))).blob_id;
    await h.db
      .prepare("UPDATE generations SET expires_at = 0, gc_after = 0 WHERE state = 'ready'")
      .run();
    assert.equal((await h.fetch(bytes('A'), bytes('S'))).status, 404);
    assert.equal((await h.store(bytes('A'), bytes('S'), bytes('newest'))).status, 200);
    assert.equal((await h.mf.dispatchFetch(`${endpoint}/blob/${expiredBlob}`)).status, 404);
    await h.db
      .prepare("UPDATE generations SET expires_at = 0, gc_after = 0 WHERE state = 'ready'")
      .run();
    await (await h.mf.getWorker()).scheduled();
    await (await h.mf.getWorker()).scheduled();
    const accounting = await h.db.prepare('SELECT * FROM deployment').first();
    assert.ok(accounting);
    assert.equal(accounting.charged_bytes, 0);
    assert.equal(accounting.entry_count, 0);
    assert.equal(accounting.association_count, 0);
    assert.equal((await h.bucket.list()).objects.length, 0);
  } finally {
    await h.close();
  }
});

void test('rate admission uses bounded keys and precedes storage', async () => {
  const h = await harness({ readLimit: 1, storeLimit: 1 });
  try {
    assert.equal((await h.mf.dispatchFetch('https://cache.example.com/unknown-one')).status, 404);
    const limited = await h.mf.dispatchFetch('https://cache.example.com/unknown-two');
    assert.equal(limited.status, 429);
    assert.equal(limited.headers.get('Retry-After'), '60');
    assert.equal((await h.mf.dispatchFetch(`${endpoint}/store`, { method: 'POST' })).status, 401);
    assert.equal((await h.mf.dispatchFetch(`${endpoint}/store`, { method: 'POST' })).status, 429);
    assert.equal((await h.db.prepare('SELECT count(*) AS n FROM generations').first())!.n, 0);
  } finally {
    await h.close();
  }
});

void test('malformed stores cannot publish and missing live value is a storage failure', async () => {
  const h = await harness();
  try {
    const metadata = encode({ key: bytes('A'), secondary_key: bytes('S'), value: bytes('V') });
    for (const ending of ['', '\r\n--bad--', '\r\n--x\r\n']) {
      const body = Buffer.concat([
        Buffer.from(
          '--x\r\nContent-Disposition: form-data; name="metadata"\r\nContent-Type: application/cbor\r\n\r\n',
        ),
        metadata,
        Buffer.from(ending),
      ]);
      const response = await h.mf.dispatchFetch(`${endpoint}/store`, {
        method: 'POST',
        headers: {
          'Content-Type': 'multipart/form-data; boundary=x',
          Authorization: `Bearer ${await h.token()}`,
        },
        body,
      });
      assert.equal(response.status, 400);
    }
    assert.equal((await h.fetch(bytes('A'), bytes('S'))).status, 404);
    await h.store(bytes('A'), bytes('S'), bytes('V'));
    const g = await h.db
      .prepare("SELECT value_object FROM generations WHERE state = 'ready'")
      .first();
    assert.ok(g);
    assert.equal(typeof g.value_object, 'string');
    await h.bucket.delete(String(g.value_object));
    assert.equal((await h.fetch(bytes('A'), bytes('S'))).status, 503);
  } finally {
    await h.close();
  }
});

void test('configured field and streamed request limits return 413 without publication', async () => {
  const h = await harness({
    limits: {
      key: 8,
      value: 8,
      metadata: 256,
      fetch: 256,
      blob: 16,
      store: 2048,
      headers: 512,
    },
  });
  try {
    for (const [key, secondary, value, blob] of [
      [new Uint8Array(9), bytes('S'), bytes('V')],
      [bytes('A'), new Uint8Array(9), bytes('V')],
      [bytes('A'), bytes('S'), new Uint8Array(9)],
      [bytes('A'), bytes('S'), bytes('V'), new Uint8Array(17)],
    ])
      assert.equal((await h.store(key!, secondary!, value!, blob)).status, 413);
    assert.equal((await h.fetch(new Uint8Array(9), bytes('S'))).status, 413);
    const token = await h.token();
    for (const body of [
      `--x\r\nContent-Disposition: form-data; name="metadata"\r\nContent-Type: application/cbor\r\n\r\n${'a'.repeat(257)}\r\n--x--`,
      'a'.repeat(2049),
    ]) {
      const response = await h.mf.dispatchFetch(`${endpoint}/store`, {
        method: 'POST',
        headers: {
          Authorization: `Bearer ${token}`,
          'Content-Type': 'multipart/form-data; boundary=x',
        },
        body: new Blob([body]).stream(),
        duplex: 'half',
      });
      assert.equal(response.status, 413);
    }
    assert.equal((await h.db.prepare('SELECT count(*) AS n FROM entries').first())!.n, 0);
    const form = new FormData();
    form.append(
      'metadata',
      new Blob([encode({ key: bytes('A'), secondary_key: bytes('S'), value: bytes('V') })], {
        type: 'application/cbor',
      }),
    );
    const request = new Request(`${endpoint}/store`, { method: 'POST', body: form });
    assert.equal(request.headers.has('Content-Length'), false);
    const response = await h.mf.dispatchFetch(request.url, {
      method: 'POST',
      headers: { ...Object.fromEntries(request.headers), Authorization: `Bearer ${token}` },
      body: request.body,
      duplex: 'half',
    });
    assert.equal(response.status, 200, await response.clone().text());
    assert.deepEqual(
      (await decodeResponse(await h.fetch(bytes('A'), bytes('S')))).value,
      bytes('V'),
    );
  } finally {
    await h.close();
  }
});
