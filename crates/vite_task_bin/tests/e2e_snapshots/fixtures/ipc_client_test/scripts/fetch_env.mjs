import { getEnv } from '@voidzero-dev/vite-task-client';

const value = getEnv('PROBE_ENV') ?? '(unset)';

console.log('PROBE_ENV=' + value);
