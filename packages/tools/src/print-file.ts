#!/usr/bin/env node

import { readFileSync } from 'node:fs';

for (const file of process.argv.slice(2)) {
  const content = readFileSync(file);
  process.stdout.write(content);
}
