import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { ignoreOutput } from '@voidzero-dev/vite-task-client';

mkdirSync('sidecar', { recursive: true });
writeFileSync('sidecar/tmp.txt', 'initial\n');
readFileSync('sidecar/tmp.txt', 'utf8');
writeFileSync('sidecar/tmp.txt', 'final\n');
ignoreOutput('sidecar');

mkdirSync('dist', { recursive: true });
writeFileSync('dist/out.txt', 'ok\n');
