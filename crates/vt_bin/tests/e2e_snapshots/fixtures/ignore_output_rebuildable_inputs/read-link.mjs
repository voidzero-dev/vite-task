import { existsSync, mkdirSync, readdirSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { ignoreOutput } from '@voidzero-dev/vite-task-client';

const present = existsSync('alias');
if (present) readdirSync('alias');

mkdirSync('real', { recursive: true });
rmSync('alias', { recursive: true, force: true });
symlinkSync('real', 'alias', 'junction');
writeFileSync('alias/child.txt', 'output\n');
ignoreOutput('alias');
writeFileSync('result.txt', present ? 'present\n' : 'missing\n');
