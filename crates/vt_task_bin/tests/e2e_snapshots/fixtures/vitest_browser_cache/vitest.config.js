import { playwright } from '@vitest/browser-playwright';
import { defineConfig } from 'vitest/config';
import { DefaultReporter } from 'vitest/node';

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
