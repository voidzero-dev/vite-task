import { mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { ignoreInput, ignoreOutput } from '@voidzero-dev/vite-task-client';

const cacheDir = 'tool/cache';
const statePath = `${cacheDir}/state.txt`;

mkdirSync(cacheDir, { recursive: true });
readdirSync('tool');
readdirSync('.');

writeFileSync(statePath, 'seed');
readFileSync(statePath, 'utf8');
if (process.argv[2] !== 'input-negative') ignoreInput(cacheDir);
ignoreOutput(cacheDir);
