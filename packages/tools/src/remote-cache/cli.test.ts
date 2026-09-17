import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { access, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import test, { type TestContext } from 'node:test';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const exec = promisify(execFile);
const cli = fileURLToPath(new URL('./cli.ts', import.meta.url));

async function fixture(t: TestContext) {
  const cwd = await mkdtemp(join(tmpdir(), 'remote-cache-test-'));
  const run = (...args: string[]) =>
    exec(process.execPath, [cli, ...args], { cwd, timeout: 15_000 });
  t.after(async () => {
    try {
      await run('stop');
    } finally {
      await rm(cwd, { recursive: true, force: true });
    }
  });
  return { cwd, run };
}

async function waitForRemoval(path: string): Promise<void> {
  const deadline = Date.now() + 10_000;
  while (true) {
    try {
      await access(path);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') return;
      throw error;
    }
    assert.ok(Date.now() < deadline, `Daemon did not remove ${path}`);
    await delay(25);
  }
}

await test('deleting cache.url stops the daemon and closes the listener', async (t) => {
  const { cwd, run } = await fixture(t);
  await run('start');
  const url = (await readFile(join(cwd, 'cache.url'), 'utf8')).trim();
  assert.equal((await fetch(`${url}/fetch`)).status, 404);
  await rm(join(cwd, 'cache.url'));
  await waitForRemoval(join(cwd, 'cache.lock'));
  await assert.rejects(fetch(`${url}/fetch`));
});

await test('maximum lifetime cleans up a daemon even when stop never runs', async (t) => {
  const { cwd, run } = await fixture(t);
  await run('start', '--max-lifetime-ms', '800');
  const url = (await readFile(join(cwd, 'cache.url'), 'utf8')).trim();
  await waitForRemoval(join(cwd, 'cache.lock'));
  await assert.rejects(access(join(cwd, 'cache.url')));
  await assert.rejects(fetch(`${url}/fetch`));
});

await test('concurrent starts have one owner and separate directories stay isolated', async (t) => {
  const a = await fixture(t);
  const b = await fixture(t);
  const starts = await Promise.allSettled([a.run('start'), a.run('start'), b.run('start')]);
  assert.equal(starts.filter((result) => result.status === 'fulfilled').length, 2);
  const urlA = (await readFile(join(a.cwd, 'cache.url'), 'utf8')).trim();
  const urlB = (await readFile(join(b.cwd, 'cache.url'), 'utf8')).trim();
  assert.notEqual(urlA, urlB);
  await a.run('stop');
  assert.equal((await fetch(`${urlB}/fetch`)).status, 404);
});

await test('startup refuses an existing URL file and releases its lock', async (t) => {
  const { cwd, run } = await fixture(t);
  await writeFile(join(cwd, 'cache.url'), 'do not overwrite');
  await assert.rejects(run('start'), /cache.url already exists/);
  assert.equal(await readFile(join(cwd, 'cache.url'), 'utf8'), 'do not overwrite');
  await assert.rejects(access(join(cwd, 'cache.lock')));
});
