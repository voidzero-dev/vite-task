import { ignoreOutput } from '@voidzero-dev/vite-task-client';
import { writeFileSync, readFileSync, mkdirSync } from 'node:fs';

// The task both reads and writes `sidecar/tmp.txt`. If the runner didn't
// treat `sidecar/` as an ignored output, the read-write overlap check would
// refuse to cache the task. `dist/out.txt` is the real output.
mkdirSync('sidecar', { recursive: true });
writeFileSync('sidecar/tmp.txt', 'initial\n');
readFileSync('sidecar/tmp.txt', 'utf8');
writeFileSync('sidecar/tmp.txt', 'final\n');
ignoreOutput('sidecar');

mkdirSync('dist', { recursive: true });
writeFileSync('dist/out.txt', 'ok\n');
