import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { once } from 'node:events';
import { createServer } from 'node:http';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const exec = promisify(execFile);
const cli = fileURLToPath(new URL('./cbor-http.ts', import.meta.url));

await test('the HTTP client translates CBOR, text errors, and raw responses to EDN', async (t) => {
  const server = createServer(async (request, response) => {
    const chunks = [];
    for await (const chunk of request) chunks.push(Buffer.from(chunk));
    if (request.url === '/cbor') {
      // RFC 8949: a map with one text key and one byte-string value.
      assert.deepEqual(Buffer.concat(chunks), Buffer.from([0xa1, 0x61, 0x61, 0x41, 0x62]));
      response.writeHead(200, { 'content-type': 'application/cbor' });
      response.end(Buffer.concat(chunks));
    } else if (request.url === '/text') {
      response.writeHead(503, { 'content-type': 'text/plain; charset=utf-8' });
      response.end('try again');
    } else if (request.url === '/raw') {
      response.writeHead(200, { 'content-type': 'application/octet-stream' });
      response.end(Buffer.from([0, 255, 128]));
    } else {
      response.writeHead(200, { 'content-type': 'application/cbor' });
      response.end(Buffer.from([0x41])); // Byte string missing its byte.
    }
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  t.after(() => {
    server.close();
    server.closeAllConnections();
  });
  const address = server.address();
  assert.ok(address && typeof address !== 'string');
  const run = (path: string, ...args: string[]) =>
    exec(process.execPath, [cli, 'POST', `http://127.0.0.1:${address.port}${path}`, ...args], {
      timeout: 15_000,
    });
  assert.equal(
    (await run('/cbor', '--cbor', '{"a": \'b\'}')).stdout.trim(),
    '{"status": 200, "content_type": "application/cbor", "body": {"a": \'b\'}}',
  );
  assert.equal(
    (await run('/text')).stdout.trim(),
    '{"status": 503, "content_type": "text/plain; charset=utf-8", "body": "try again"}',
  );
  assert.equal(
    (await run('/raw')).stdout.trim(),
    '{"status": 200, "content_type": "application/octet-stream", "body": b64\'AP+A\'}',
  );
  await assert.rejects(run('/invalid'), (error: unknown) => {
    assert.equal((error as { code: number }).code, 1);
    return true;
  });
});
