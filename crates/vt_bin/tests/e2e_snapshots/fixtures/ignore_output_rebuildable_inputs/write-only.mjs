import { readdirSync, writeFileSync } from 'node:fs';
import { ignoreOutput } from '@voidzero-dev/vite-task-client';

readdirSync('.');
writeFileSync('scratch.txt', 'scratch\n');
ignoreOutput('scratch.txt');
