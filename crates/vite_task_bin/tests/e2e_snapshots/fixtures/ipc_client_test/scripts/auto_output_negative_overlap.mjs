import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';

mkdirSync('scratch', { recursive: true });
mkdirSync('dist', { recursive: true });

writeFileSync('scratch/overlap.txt', 'before\n');
readFileSync('scratch/overlap.txt', 'utf8');
writeFileSync('scratch/overlap.txt', 'after\n');
writeFileSync('dist/negative-overlap.txt', 'keep\n');
