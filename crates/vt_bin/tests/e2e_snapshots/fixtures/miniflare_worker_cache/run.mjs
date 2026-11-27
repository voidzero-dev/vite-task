import fs from 'node:fs';
import { convertV4MiniflareOptions, Miniflare } from 'miniflare';

const mf = new Miniflare(
  convertV4MiniflareOptions({
    unsafeDevRegistryPath: '',
    unsafeRegisterWorker: false,
    host: '127.0.0.1',
    port: 0,
    modules: [
      { type: 'ESModule', path: 'src/worker.mjs' },
      { type: 'ESModule', path: 'src/data.mjs' },
    ],
    compatibilityDate: '2025-01-01',
  }),
);
try {
  const res = await mf.dispatchFetch('http://localhost/');
  const text = await res.text();
  fs.mkdirSync('dist', { recursive: true });
  fs.writeFileSync('dist/out.txt', `${text}\n`);
} finally {
  await mf.dispose();
}
