import { disableCache } from '@voidzero-dev/vite-task-client';
import { writeFileSync, mkdirSync } from 'node:fs';

// Produce an output, then ask the runner not to cache this execution — the
// next `vt run` should re-execute the task.
mkdirSync('dist', { recursive: true });
writeFileSync('dist/out.txt', 'ok\n');
disableCache();
