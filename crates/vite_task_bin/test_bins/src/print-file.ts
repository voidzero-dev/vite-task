#!/usr/bin/env node

import { readFileSync } from 'node:fs';

const content = readFileSync(process.argv[2]);
process.stdout.write(content);
