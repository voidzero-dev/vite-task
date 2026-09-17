import assert from 'node:assert/strict';
import { once } from 'node:events';
import { request } from 'node:http';
import test, { type TestContext } from 'node:test';
import { decode } from 'cbor2/decoder';
import { encode } from 'cbor2/encoder';
import { createCacheServer } from './server.ts';

async function backend(t: TestContext, maxRequestBytes?: number): Promise<string> {
  const server = createCacheServer(maxRequestBytes === undefined ? {} : { maxRequestBytes });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  t.after(async () => {
    const closed = once(server, 'close');
    server.close();
    server.closeAllConnections();
    await closed;
  });
  const address = server.address();
  assert.ok(address && typeof address !== 'string');
  return `http://127.0.0.1:${address.port}`;
}

function data(value: string): Uint8Array<ArrayBuffer> {
  return new TextEncoder().encode(value);
}

async function lookup(url: string, key: Uint8Array, secondary = data('S')) {
  const response = await fetch(`${url}/fetch`, {
    method: 'POST',
    headers: { 'content-type': 'application/cbor' },
    body: Buffer.from(encode({ key, secondary_key: secondary })),
  });
  assert.equal(response.status, 200);
  return decode<{ kind: string; value: Uint8Array; blob_id: string }>(
    new Uint8Array(await response.arrayBuffer()),
  );
}

await test('concurrent stores keep values paired with immutable blobs', async (t) => {
  const url = await backend(t);
  const responses = await Promise.all(
    Array.from({ length: 12 }, async (_, index) => {
      const bytes = data(`value-${index}`);
      const form = new FormData();
      form.set(
        'metadata',
        new Blob(
          [Buffer.from(encode({ key: data('A'), secondary_key: data('S'), value: bytes }))],
          { type: 'application/cbor' },
        ),
      );
      form.set('blob', new Blob([bytes], { type: 'application/octet-stream' }));
      const response = await fetch(`${url}/store`, { method: 'POST', body: form });
      assert.equal(response.status, 200);
      const { blob_id } = decode<{ blob_id: string }>(new Uint8Array(await response.arrayBuffer()));
      return { bytes, blob_id };
    }),
  );
  const exact = await lookup(url, data('A'));
  assert.equal(exact.kind, 'exact');
  assert.ok(exact.value instanceof Uint8Array);
  const downloaded = new Uint8Array(
    await (await fetch(`${url}/blob/${exact.blob_id}`)).arrayBuffer(),
  );
  assert.deepEqual(downloaded, exact.value);
  for (const { bytes, blob_id } of responses) {
    const response = await fetch(`${url}/blob/${blob_id}`);
    assert.equal(response.status, 200);
    assert.deepEqual(new Uint8Array(await response.arrayBuffer()), bytes);
  }
});

await test('large keys and values are accepted without implicit multipart field limits', async (t) => {
  const url = await backend(t);
  const key = data('k'.repeat(128 * 1024));
  const value = data('v'.repeat(2 * 1024 * 1024));
  const form = new FormData();
  form.set(
    'metadata',
    new Blob([Buffer.from(encode({ key, secondary_key: key, value }))], {
      type: 'application/cbor',
    }),
  );
  assert.equal((await fetch(`${url}/store`, { method: 'POST', body: form })).status, 200);
  assert.deepEqual((await lookup(url, key, key)).value, value);
});

await test('an aborted upload never publishes metadata received before the blob', async (t) => {
  const url = await backend(t);
  const first = new FormData();
  first.set(
    'metadata',
    new Blob(
      [Buffer.from(encode({ key: data('A'), secondary_key: data('S'), value: data('original') }))],
      { type: 'application/cbor' },
    ),
  );
  assert.equal((await fetch(`${url}/store`, { method: 'POST', body: first })).status, 200);

  const upload = request(`${url}/store`, {
    method: 'POST',
    headers: { 'content-type': 'multipart/form-data; boundary=test' },
  });
  upload.on('error', () => {}); // Aborting our own socket is intentional.
  upload.write(
    '--test\r\nContent-Disposition: form-data; name="metadata"\r\nContent-Type: application/cbor\r\n\r\n',
  );
  upload.write(encode({ key: data('A'), secondary_key: data('T'), value: data('replacement') }));
  upload.write(
    '\r\n--test\r\nContent-Disposition: form-data; name="blob"\r\nContent-Type: application/octet-stream\r\n\r\n',
  );
  await new Promise<void>((resolve) => upload.write('partial blob', () => resolve()));
  const closed = new Promise<void>((resolve) => upload.once('close', resolve));
  upload.destroy();
  await closed;
  assert.deepEqual((await lookup(url, data('A'))).value, data('original'));
  const response = await fetch(`${url}/fetch`, {
    method: 'POST',
    headers: { 'content-type': 'application/cbor' },
    body: Buffer.from(encode({ key: data('B'), secondary_key: data('T') })),
  });
  assert.equal(response.status, 404);
});

await test('chunked requests exceeding the limit return 413 without Content-Length', async (t) => {
  const url = await backend(t, 32);
  const upload = request(`${url}/fetch`, {
    method: 'POST',
    headers: { 'content-type': 'application/cbor' },
  });
  const result = new Promise<number | undefined>((resolve, reject) => {
    upload.on('error', reject);
    upload.on('response', (response) => {
      response.resume();
      resolve(response.statusCode);
    });
  });
  upload.write(Buffer.alloc(20));
  upload.end(Buffer.alloc(20));
  assert.equal(await result, 413);
});
