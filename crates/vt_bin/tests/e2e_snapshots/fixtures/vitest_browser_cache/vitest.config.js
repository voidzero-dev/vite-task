import { playwright } from 'vite-plus/test/browser-playwright';
import { defineConfig } from 'vite-plus';
import { DefaultReporter } from 'vite-plus/test/node';

class NoTestSummaryReporter extends DefaultReporter {
  reportTestSummary() {}
}

export default defineConfig({
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
