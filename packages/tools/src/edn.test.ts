import assert from 'node:assert/strict';
import test from 'node:test';
import { parseEDN } from 'cbor-edn';
import { decode } from 'cbor2/decoder';
import { encode } from 'cbor2/encoder';
import { toEDN } from './edn.ts';

await test('EDN distinguishes binary and text, including escapes and invalid UTF-8', () => {
  for (const value of [
    new Uint8Array(),
    new Uint8Array([0, 255, 128]),
    new TextEncoder().encode("quotes: '\"\\\n\t\0 and h'ff'"),
    new Uint8Array([239, 187, 191, 65]),
    { text: "h'ff'", bytes: new TextEncoder().encode('hello') },
    { text: '"quoted"', values: [true, 1, 3.5, null] },
  ]) {
    const edn = toEDN(encode(value));
    assert.deepEqual(decode(parseEDN(edn, {})), value);
  }
  assert.equal(toEDN(encode(new Uint8Array([0, 255, 128]))), "b64'AP+A'");
  assert.equal(toEDN(encode(new TextEncoder().encode('hello'))), "'hello'");
});

await test('known RFC 8949 encodings decode without depending on our request encoder', () => {
  assert.equal(toEDN(Uint8Array.of(0x43, 0x61, 0x62, 0x63)), "'abc'");
  assert.equal(toEDN(Uint8Array.of(0x63, 0x61, 0x62, 0x63)), '"abc"');
  assert.equal(toEDN(Uint8Array.of(0xa1, 0x61, 0x61, 0x01)), '{"a": 1}');
});
