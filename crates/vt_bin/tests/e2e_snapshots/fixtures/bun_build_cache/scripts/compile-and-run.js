import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'bun-compile-test-'));
try {
  const exePath = path.join(tmpDir, process.platform === 'win32' ? 'app.exe' : 'app');
  const compile = spawnSync(
    process.execPath,
    ['build', '--compile', 'src/cli.js', '--outfile', exePath],
    {
      encoding: 'utf8',
    },
  );
  if (compile.status !== 0) {
    process.stderr.write(compile.stdout + compile.stderr);
    process.exit(compile.status ?? 1);
  }
  const run = spawnSync(exePath, ['src/data.txt', 'dist/out.txt'], {
    encoding: 'utf8',
  });
  if (run.status !== 0) {
    process.stderr.write(run.stdout + run.stderr);
    process.exit(run.status ?? 1);
  }
} finally {
  fs.rmSync(tmpDir, { recursive: true, force: true });
}
