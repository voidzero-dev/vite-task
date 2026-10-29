import { defineConfig } from 'vitest/config';
import { DefaultReporter } from 'vitest/node';

class NoTestSummaryReporter extends DefaultReporter {
  reportTestSummary() {}
}

export default defineConfig({
  test: {
    reporters: [new NoTestSummaryReporter({ summary: false })],
    fsModuleCache: true,
  },
});
