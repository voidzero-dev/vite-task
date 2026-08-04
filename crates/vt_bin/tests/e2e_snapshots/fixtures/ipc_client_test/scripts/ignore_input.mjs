import { mkdirSync, readFileSync } from 'node:fs';
import { ignoreInput } from '@voidzero-dev/vite-task-client';

mkdirSync('cache_like', { recursive: true });

ignoreInput('cache_like');

const value = readFileSync('cache_like/input.txt', 'utf8').trim();
console.log(value);
