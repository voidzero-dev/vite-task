// Simulates Vite's behavior during build:
// 1. Reads the source file (tracked as input by fspy)
// 2. Writes a temp config to node_modules/.vite-temp/ (also tracked by fspy)
// Without the .vite-temp exclusion, step 2 causes a read-write overlap
// that prevents caching.

import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';

// Read source (tracked as input)
const source = readFileSync('src/index.ts', 'utf-8');

// Simulate Vite writing temp config (read-write to .vite-temp)
const tempDir = 'node_modules/.vite-temp';
mkdirSync(tempDir, { recursive: true });
const tempFile = `${tempDir}/vite.config.ts.timestamp-${Date.now()}.mjs`;
writeFileSync(tempFile, `// compiled config\nexport default {};`);

console.log(`built: ${source.trim()}`);
