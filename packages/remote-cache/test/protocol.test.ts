import assert from 'node:assert/strict';
import { test } from 'node:test';
import { decode, encode } from 'cborg';
import { decodeEnvelope, encodeEnvelope } from '../src/cbor.ts';
import { defaults } from '../src/limits.ts';
import { multipart } from '../src/multipart.ts';
import { collect, Deadline, Input } from '../src/streams.ts';
import fixtures from './fixtures/protocol.json';

void test('CBOR fixtures preserve opaque, empty, indefinite, and noncanonical bytes', () => {
  for (const fixture of fixtures.fetch) {
    const actual = decodeEnvelope(Buffer.from(fixture.hex, 'hex'), false, defaults);
    assert.deepEqual(Array.from(actual.key), fixtures.expectedKey, fixture.name);
    assert.deepEqual(Array.from(actual.secondary_key), fixtures.expectedSecondaryKey, fixture.name);
  }
  const value = Uint8Array.of(0, 255, 159, 255);
  assert.deepEqual(decode(encodeEnvelope({ kind: 'exact', value, blob_id: null })), {
    kind: 'exact',
    value,
    blob_id: null,
  });
});

void test('multipart accepts bounded preamble, padding, and epilogue and rejects invalid parts', async () => {
  const headers =
    'Content-Disposition: form-data; name="metadata"\r\nContent-Type: application/cbor\r\n\r\n';
  const valid = `preamble\r\n--x \t\r\n${headers}abc\r\n--x-- \t\r\nepilogue`;
  async function parse(data: string, limit = 1024) {
    const deadline = new Deadline(5000);
    const input = new Input(new Blob([data]).stream(), 4096, deadline);
    try {
      const parts = [];
      for await (const part of multipart(input, 'x', limit))
        parts.push(Buffer.from(await collect(part.body, 1024)).toString());
      return parts;
    } finally {
      input.close();
      deadline.dispose();
    }
  }
  assert.deepEqual(await parse(valid), ['abc']);
  for (const data of [
    `--x\r\n${headers}abc\r\n--x\r\n${headers}abc\r\n--x--`,
    `--x\r\n${headers.replace('metadata', 'unknown')}abc\r\n--x--`,
    `--x\r\n${headers.replace('application/cbor', 'text/plain')}abc\r\n--x--`,
    `--x\r\n${headers}abc\r\n--x--invalid`,
  ])
    await assert.rejects(parse(data), { status: 400 });
  await assert.rejects(parse('a'.repeat(1025) + '\r\n' + valid), { status: 413 });
});

void test('CBOR rejects duplicate, missing, wrong-type, nested, oversized, and trailing fields', () => {
  for (const hex of [
    'a2636b657940636b657940',
    'a1636b657940',
    'a2636b6579606d7365636f6e646172795f6b657940',
    'a2636b657981406d7365636f6e646172795f6b657940',
    'a2636b65795bffffffffffffffff',
    'a2636b6579406d7365636f6e646172795f6b65794000',
  ]) {
    assert.throws(() => decodeEnvelope(Buffer.from(hex, 'hex'), false, defaults));
  }
  assert.throws(
    () =>
      decodeEnvelope(
        encode({ key: new Uint8Array(defaults.key + 1), secondary_key: new Uint8Array() }),
        false,
        defaults,
      ),
    { status: 413 },
  );
});

void test('multipart accepts every split point, either part order, and boundary-like blob bytes', async () => {
  for (const blobFirst of [false, true]) {
    const metadata =
      '--test\r\nContent-Disposition: form-data; name="metadata"\r\nContent-Type: application/cbor\r\n\r\nabc\r\n';
    const blob =
      '--test\r\nContent-Disposition: form-data; name="blob"; filename="out"\r\nContent-Type: application/octet-stream\r\n\r\nx\r\n--testXYz\r\n--test--XYz\r\n';
    const data = Buffer.from((blobFirst ? blob + metadata : metadata + blob) + '--test--\r\n');
    for (let split = 1; split < data.length; split++) {
      const stream = new ReadableStream({
        start(controller) {
          controller.enqueue(data.subarray(0, split));
          controller.enqueue(data.subarray(split));
          controller.close();
        },
      });
      const deadline = new Deadline(5000);
      const input = new Input(stream, 4096, deadline);
      try {
        const result: Record<string, string> = {};
        for await (const part of multipart(input, 'test', 1024))
          result[part.name] = Buffer.from(await collect(part.body, 1024)).toString();
        assert.deepEqual(result, { metadata: 'abc', blob: 'x\r\n--testXYz\r\n--test--XYz' });
      } finally {
        deadline.dispose();
        input.close();
      }
    }
  }
});
