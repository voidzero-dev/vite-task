// Programmatic Vite dev server bring-up: middleware mode skips the HTTP
// listen entirely (Windows runners refuse the 127.0.0.1 bind with
// `listen UNKNOWN`), but `_createServer` still calls `disableCache()`
// via `@voidzero-dev/vite-task-client` on its first line — so even
// though this process exits 0 the runner is told not to store the run
// and the next invocation must miss.
import { createServer } from 'vite';

const server = await createServer({
  configFile: false,
  logLevel: 'silent',
  server: { middlewareMode: true },
});
await server.close();
