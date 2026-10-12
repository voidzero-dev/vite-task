import { playwright } from '@vitest/browser-playwright';
import { defineConfig } from 'vitest/config';
import { DefaultReporter } from 'vitest/node';

class NoTestSummaryReporter extends DefaultReporter {
  reportTestSummary() {}
}

export default defineConfig({
  plugins: [
    {
      name: 'browser-server-port',
      // Vitest's fixed default browser server port (63315) can fall inside a
      // Windows excluded port range, where listening fails with EACCES and
      // Vite does not fall back to another port. Port 0 lets the OS choose.
      // Vitest replaces `test.api.port: 0` with the default and prints an
      // "API started at" line when `test.api` is set, so override the port
      // after Vitest resolves `test.api`.
      enforce: 'post',
      config: {
        order: 'post',
        handler(config) {
          config.test.api.port = 0;
        },
      },
    },
  ],
  test: {
    reporters: [new NoTestSummaryReporter({ summary: false }), 'json'],
    outputFile: { json: 'dist/result.json' },
    browser: {
      enabled: true,
      headless: true,
      provider: playwright(),
      instances: [{ browser: 'chromium' }],
    },
  },
});
