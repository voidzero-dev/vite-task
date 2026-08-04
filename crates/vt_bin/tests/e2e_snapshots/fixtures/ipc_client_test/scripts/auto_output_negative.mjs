import { mkdirSync, writeFileSync } from 'node:fs';

mkdirSync('dist', { recursive: true });
writeFileSync('dist/keep.txt', 'keep\n');
writeFileSync('dist/skip.txt', 'skip\n');
