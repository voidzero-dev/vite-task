import assert from 'node:assert/strict';
import test from 'node:test';
import { parseEDN } from 'cbor-edn';
import { decode } from 'cbor2/decoder';
import { encode } from 'cbor2/encoder';
import { toEDN } from './edn.ts';

await test('EDN formatting preserves escapes, a UTF-8 BOM, and text resembling byte strings', () => {
  for (const value of [
    new TextEncoder().encode("quotes: '\"\\\n\t\0 and h'ff'"),
    new Uint8Array([239, 187, 191, 65]),
    { text: "h'ff'", bytes: new TextEncoder().encode('hello') },
  ]) {
    const edn = toEDN(encode(value));
    assert.deepEqual(decode(parseEDN(edn, {})), value);
  }
});
