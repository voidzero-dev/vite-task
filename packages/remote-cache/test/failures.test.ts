import assert from 'node:assert/strict';
import { test } from 'node:test';
import { encode } from 'cborg';
import { harness, bytes, decodeResponse } from './helpers.ts';
import { cleanup, getScope, reserve, publish } from '../src/database.ts';
import { store } from '../src/store.ts';
import { defaults } from '../src/limits.ts';
import { Deadline } from '../src/streams.ts';
import { Observations } from '../src/observations.ts';
import { Admission } from '../src/admission.ts';

function multipartRequest(blob?: Uint8Array): Request {
  const form = new FormData();
  form.append(
    'metadata',
    new Blob([encode({ key: bytes('A'), secondary_key: bytes('T'), value: bytes('new') })], {
      type: 'application/cbor',
    }),
  );
  if (blob) form.append('blob', new Blob([blob], { type: 'application/octet-stream' }));
  return new Request('https://cache.example.com/projects/test/store', {
    method: 'POST',
    body: form,
  });
}

void test('R2 PUT, multipart creation, part, completion, and upload-ID recording failures leave mappings unchanged', async () => {
  const h = await harness();
  try {
    await h.store(bytes('A'), bytes('S'), bytes('old'));
    const original = await h.mf.getBindings<Env>();
    for (const failure of ['value', 'blob', 'create', 'part', 'complete', 'record']) {
      const pending: Promise<unknown>[] = [];
      const deadline = new Deadline(5000);
      const bucket = new Proxy(original.ARTIFACTS, {
        get(target, prop) {
          if (prop === 'put')
            return async (key: string, value: ArrayBuffer | ArrayBufferView) => {
              if (key.endsWith(`/${failure}`)) throw new Error('Injected PUT failure');
              return target.put(key, value);
            };
          if (prop === 'createMultipartUpload')
            return async (key: string) => {
              if (failure === 'create') throw new Error('Injected multipart creation failure');
              const upload = await target.createMultipartUpload(key);
              if (failure === 'record')
                await h.db
                  .prepare("UPDATE generations SET lease_until = 0 WHERE state = 'uploading'")
                  .run();
              return {
                key: upload.key,
                uploadId: upload.uploadId,
                abort: () => upload.abort(),
                uploadPart: async (number: number, value: ArrayBufferView) => {
                  if (failure === 'part') throw new Error('Injected part failure');
                  return upload.uploadPart(number, value);
                },
                complete: async (parts: R2UploadedPart[]) => {
                  if (failure === 'complete') throw new Error('Injected completion failure');
                  return upload.complete(parts);
                },
              };
            };
          const value = Reflect.get(target, prop);
          return typeof value === 'function' ? value.bind(target) : value;
        },
      });
      try {
        const blob = new Uint8Array(['value', 'blob'].includes(failure) ? 4 : 5 * 1024 * 1024 + 1);
        await assert.rejects(
          store(
            multipartRequest(blob),
            { ...original, ARTIFACTS: bucket },
            { waitUntil: (promise) => pending.push(promise) },
            await getScope(h.db, 'test'),
            { exp: Math.floor(Date.now() / 1000) + 300, repository_id: '123' },
            defaults,
            deadline,
            new Observations(),
          ),
        );
        await Promise.all(pending);
        assert.deepEqual(
          (await decodeResponse(await h.fetch(bytes('A'), bytes('S')))).value,
          bytes('old'),
        );
        assert.equal((await h.fetch(bytes('missing'), bytes('T'))).status, 404);
      } finally {
        deadline.dispose();
      }
    }
    await h.db.prepare("UPDATE generations SET gc_after = 0 WHERE state = 'uploading'").run();
    await cleanup(original);
    assert.equal((await h.db.prepare('SELECT count(*) AS n FROM generations').first())!.n, 1);
    assert.equal((await h.bucket.list()).objects.length, 1);
  } finally {
    await h.close();
  }
});

void test('failed deletion remains charged and a retry removes only the claimed generation', async () => {
  const h = await harness();
  try {
    await h.store(bytes('A'), bytes('S'), bytes('old'), bytes('blob'));
    await h.store(bytes('A'), bytes('T'), bytes('new'));
    await h.db.prepare("UPDATE generations SET gc_after = 0 WHERE state = 'retired'").run();
    const original = await h.mf.getBindings<Env>();
    const bucket = new Proxy(original.ARTIFACTS, {
      get(target, prop) {
        if (prop === 'delete')
          return async () => {
            throw new Error('Injected deletion failure');
          };
        const value = Reflect.get(target, prop);
        return typeof value === 'function' ? value.bind(target) : value;
      },
    });
    await cleanup({ ...original, ARTIFACTS: bucket });
    assert.equal(
      (await h.db.prepare('SELECT charged_bytes FROM deployment').first())!.charged_bytes,
      10,
    );
    await h.db.prepare("UPDATE generations SET gc_after = 0 WHERE state = 'deleting'").run();
    await cleanup(original);
    assert.equal(
      (await h.db.prepare('SELECT charged_bytes FROM deployment').first())!.charged_bytes,
      3,
    );
    assert.deepEqual(
      (await decodeResponse(await h.fetch(bytes('missing'), bytes('S')))).value,
      bytes('new'),
    );
  } finally {
    await h.close();
  }
});

void test('unknown-length requests reserve the full limit and cancellation prevents publication', async () => {
  const h = await harness();
  try {
    const env = await h.mf.getBindings<Env>();
    const pending: Promise<unknown>[] = [];
    const deadline = new Deadline(100);
    let cancelled = false;
    const requestInit = {
      method: 'POST',
      headers: { 'Content-Type': 'multipart/form-data; boundary=x' },
      body: new ReadableStream({
        cancel() {
          cancelled = true;
        },
      }),
      duplex: 'half',
    };
    const request = new Request('https://cache.example.com/projects/test/store', requestInit);
    try {
      await assert.rejects(
        store(
          request,
          env,
          { waitUntil: (promise) => pending.push(promise) },
          await getScope(h.db, 'test'),
          { exp: Math.floor(Date.now() / 1000) + 300, repository_id: '123' },
          defaults,
          deadline,
          new Observations(),
        ),
        { status: 503 },
      );
      await Promise.all(pending);
      assert.equal(cancelled, true);
      const row = await h.db.prepare('SELECT charged_bytes, state FROM generations').first();
      assert.equal(row!.charged_bytes, defaults.store);
      assert.equal(row!.state, 'uploading');
      assert.equal((await h.fetch(bytes('A'), bytes('T'))).status, 404);
    } finally {
      deadline.dispose();
    }
  } finally {
    await h.close();
  }
});

void test('scope/deployment disable and capacity changes revoke pending publications', async () => {
  const h = await harness();
  try {
    for (const sql of [
      'UPDATE scopes SET enabled = 0',
      'UPDATE scopes SET writes_enabled = 0',
      'UPDATE scopes SET retention_seconds = retention_seconds + 1',
      'UPDATE deployment SET enabled = 0',
      'UPDATE deployment SET writes_enabled = 0',
      'UPDATE deployment SET byte_limit = 1',
    ]) {
      const g = await reserve(
        h.db,
        await getScope(h.db, 'test'),
        Math.floor(Date.now() / 1000) + 300,
        100,
      );
      await h.db.prepare(sql).run();
      await assert.rejects(publish(h.db, g, bytes('A'), bytes('S')), { status: 503 });
      await h.db.prepare('UPDATE scopes SET enabled = 1, writes_enabled = 1').run();
      await h.db
        .prepare('UPDATE deployment SET enabled = 1, writes_enabled = 1, byte_limit = 8000000000')
        .run();
    }
    assert.equal((await h.db.prepare('SELECT count(*) AS n FROM entries').first())!.n, 0);
    assert.equal((await h.db.prepare('SELECT count(*) AS n FROM associations').first())!.n, 0);
  } finally {
    await h.close();
  }
});

void test('scope and deployment entry limits roll back association changes, while replacements use no new slot', async () => {
  const h = await harness();
  try {
    assert.equal((await h.store(bytes('A'), bytes('S'), bytes('old'))).status, 200);
    for (const table of ['scopes', 'deployment']) {
      await h.db.prepare(`UPDATE ${table} SET entry_limit = 1`).run();
      assert.equal((await h.store(bytes('B'), bytes('S'), bytes('rejected'))).status, 503);
      assert.deepEqual(
        (await decodeResponse(await h.fetch(bytes('missing'), bytes('S')))).key,
        bytes('A'),
      );
      assert.equal((await h.fetch(bytes('B'), bytes('missing'))).status, 404);
      assert.equal((await h.store(bytes('A'), bytes('S'), bytes('replacement'))).status, 200);
      await h.db.prepare(`UPDATE ${table} SET association_limit = 1`).run();
      assert.equal((await h.store(bytes('A'), bytes('T'), bytes('rejected'))).status, 503);
      assert.deepEqual(
        (await decodeResponse(await h.fetch(bytes('A'), bytes('S')))).value,
        bytes('replacement'),
      );
      await h.db
        .prepare(`UPDATE ${table} SET entry_limit = 20000, association_limit = 20000`)
        .run();
    }
    const counters = await h.db
      .prepare('SELECT entry_count, association_count FROM deployment')
      .first();
    assert.deepEqual(counters, { entry_count: 1, association_count: 1 });
  } finally {
    await h.close();
  }
});

void test('cleanup preserves a recreated target and claims at most 16 generations', async () => {
  const h = await harness();
  try {
    await h.store(bytes('A'), bytes('S'), bytes('old'), bytes('old'));
    await h.db.prepare('UPDATE generations SET expires_at = 0, gc_after = 0').run();
    const original = await h.mf.getBindings<Env>();
    const bucket = new Proxy(original.ARTIFACTS, {
      get(target, prop) {
        if (prop === 'delete')
          return async (keys: string[]) => {
            assert.equal((await h.store(bytes('A'), bytes('T'), bytes('new'))).status, 200);
            const claimed = await h.db
              .prepare("SELECT count(*) AS n FROM generations WHERE state = 'deleting'")
              .first();
            assert.equal(claimed!.n, 1);
            await target.delete(keys);
          };
        const value = Reflect.get(target, prop);
        return typeof value === 'function' ? value.bind(target) : value;
      },
    });
    await cleanup({ ...original, ARTIFACTS: bucket });
    assert.deepEqual(
      (await decodeResponse(await h.fetch(bytes('missing'), bytes('S')))).value,
      bytes('new'),
    );
    const scope = await getScope(h.db, 'test');
    for (let i = 0; i < 18; i++)
      await reserve(h.db, scope, Math.floor(Date.now() / 1000) + 300, 100);
    await h.db.prepare("UPDATE generations SET gc_after = 0 WHERE state = 'uploading'").run();
    await cleanup(original);
    assert.equal((await h.db.prepare('SELECT count(*) AS n FROM generations').first())!.n, 3);
    assert.equal(
      (await h.db.prepare('SELECT charged_bytes FROM deployment').first())!.charged_bytes,
      203,
    );
    await cleanup(original);
    assert.equal((await h.db.prepare('SELECT count(*) AS n FROM generations').first())!.n, 1);
    assert.equal(
      (await h.db.prepare('SELECT charged_bytes FROM deployment').first())!.charged_bytes,
      3,
    );
  } finally {
    await h.close();
  }
});

void test('isolate admission bounds memory and releases capacity', () => {
  const admission = new Admission();
  const first = admission.acquire(true),
    second = admission.acquire(true);
  assert.throws(() => admission.acquire(true), { status: 503 });
  first();
  const third = admission.acquire(true);
  second();
  third();
  const readers = Array.from({ length: 4 }, () => admission.acquire(false));
  assert.throws(() => admission.acquire(false), { status: 503 });
  for (const release of readers) release();
});
