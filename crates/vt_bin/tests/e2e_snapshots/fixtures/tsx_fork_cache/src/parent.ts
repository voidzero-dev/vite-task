import { fork } from 'node:child_process';
import fs from 'node:fs';
import { fileURLToPath } from 'node:url';

const childPath: string = fileURLToPath(new URL('./child.ts', import.meta.url));
const child = fork(childPath, ['src/data.txt']);

let received: string | null = null;
child.on('message', (msg: unknown) => {
  received = String(msg);
});
child.on('exit', (code: number | null) => {
  if (code !== 0 || received === null) {
    process.exit(1);
  }
  fs.mkdirSync('dist', { recursive: true });
  fs.writeFileSync('dist/out.txt', `ipc:${received}\n`);
});
