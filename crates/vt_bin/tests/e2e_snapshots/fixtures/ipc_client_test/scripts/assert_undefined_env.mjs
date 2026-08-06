import { getEnv } from '@voidzero-dev/vite-task-client';

const name = '__VITE_TASK_CLIENT_MISSING_ENV__';
const value = getEnv(name);

if (value !== undefined) {
  throw new Error(`expected ${name} to be undefined, got ${value}`);
}

console.log('missing undefined');
