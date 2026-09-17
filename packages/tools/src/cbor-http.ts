#!/usr/bin/env node
import { parseEDN } from 'cbor-edn';
import { decode } from 'cbor2/decoder';
import { encode } from 'cbor2/encoder';
import { randomUUID } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { parseArgs } from 'node:util';
import { toEDN } from './edn.ts';

function bytes(edn: string): Uint8Array {
  const value: unknown = decode(parseEDN(edn, {}));
  if (!(value instanceof Uint8Array)) throw new Error('Expected an EDN byte string');
  return value;
}

async function main(): Promise<void> {
  const { values, positionals, tokens } = parseArgs({
    allowPositionals: true,
    tokens: true,
    options: {
      cbor: { type: 'string' },
      data: { type: 'string' },
      'content-type': { type: 'string' },
      'form-cbor': { type: 'string', multiple: true },
      'form-data': { type: 'string', multiple: true },
      'form-file': { type: 'string', multiple: true },
    },
  });
  const [method, path] = positionals;
  if (positionals.length !== 2 || !method || !path) {
    throw new Error(
      'Usage: cbor-http METHOD PATH [--cbor EDN | --data EDN | --form-cbor NAME=EDN | --form-data NAME=EDN | --form-file NAME=FILE]',
    );
  }
  let body: Buffer | undefined;
  let contentType: string | undefined;
  const formTokens = tokens.filter(
    (token) => token.kind === 'option' && token.name.startsWith('form-'),
  );
  if (
    [values.cbor !== undefined, values.data !== undefined, formTokens.length > 0].filter(Boolean)
      .length > 1
  ) {
    throw new Error('Choose one request body format');
  }
  if (values.cbor !== undefined) {
    body = Buffer.from(parseEDN(values.cbor, {}));
    contentType = 'application/cbor';
  } else if (values.data !== undefined) {
    body = Buffer.from(bytes(values.data));
    contentType = 'application/octet-stream';
  } else if (formTokens.length) {
    const boundary = randomUUID();
    const chunks: Buffer[] = [];
    for (const token of formTokens) {
      if (token.kind !== 'option' || token.value === undefined) continue;
      const equal = token.value.indexOf('=');
      const name = token.value.slice(0, equal);
      const value = token.value.slice(equal + 1);
      if (equal < 1 || /["\r\n\\]/.test(name)) throw new Error('Expected multipart NAME=VALUE');
      const isCbor = token.name === 'form-cbor';
      const data = isCbor
        ? parseEDN(value, {})
        : token.name === 'form-file'
          ? await readFile(value)
          : bytes(value);
      // Do not add filenames: binary metadata is valid without one.
      chunks.push(
        Buffer.from(
          `--${boundary}\r\nContent-Disposition: form-data; name="${name}"\r\nContent-Type: ${isCbor ? 'application/cbor' : 'application/octet-stream'}\r\n\r\n`,
        ),
      );
      chunks.push(Buffer.from(data), Buffer.from('\r\n'));
    }
    chunks.push(Buffer.from(`--${boundary}--\r\n`));
    body = Buffer.concat(chunks);
    contentType = `multipart/form-data; boundary=${boundary}`;
  }
  contentType = values['content-type'] ?? contentType;
  const url = /^https?:\/\//.test(path)
    ? path
    : `${(await readFile('cache.url', 'utf8')).trim().replace(/\/$/, '')}/${path.replace(/^\//, '')}`;
  const response = await fetch(url, {
    method,
    ...(body === undefined ? {} : { body }),
    ...(contentType === undefined ? {} : { headers: { 'content-type': contentType } }),
  });
  const responseType = response.headers.get('content-type') ?? '';
  const raw = new Uint8Array(await response.arrayBuffer());
  const type = responseType.split(';')[0]?.trim();
  let printedBody: string;
  if (type === 'application/cbor') {
    decode(raw); // diagnose alone does not reject every incomplete CBOR item.
    printedBody = toEDN(raw);
  } else {
    printedBody = toEDN(encode(type?.startsWith('text/') ? new TextDecoder().decode(raw) : raw));
  }
  console.log(
    `{"status": ${response.status}, "content_type": ${JSON.stringify(responseType)}, "body": ${printedBody}}`,
  );
}

try {
  await main();
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}
