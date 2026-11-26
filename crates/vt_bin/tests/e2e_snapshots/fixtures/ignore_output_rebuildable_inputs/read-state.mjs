import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { ignoreOutput } from '@voidzero-dev/vite-task-client';

const state = existsSync('state.json')
  ? JSON.parse(readFileSync('state.json', 'utf8'))
  : { value: 'missing' };

ignoreOutput('state.json');
writeFileSync('state.json', JSON.stringify(state) + '\n');
writeFileSync('result.json', JSON.stringify({ read: state.value }) + '\n');
