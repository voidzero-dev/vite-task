import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

const client = new URL('../src/index.js', import.meta.url).href;
const script = `import { reportUnchanged } from ${JSON.stringify(client)}; reportUnchanged(); reportUnchanged();`;

function invoke(addon) {
  const env = { ...process.env };
  delete env['VP_RUN_IPC_NAME'];
  delete env['VP_RUN_NODE_CLIENT_PATH'];
  if (addon) env['VP_RUN_NODE_CLIENT_PATH'] = addon;
  return spawnSync(process.execPath, ['--input-type=module', '-e', script], {
    env,
    encoding: 'utf8',
  });
}

test('report is a no-op outside the runner', () => {
  const result = invoke();
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, '');
});

test('report is a no-op with an older addon', () => {
  const dir = mkdtempSync(join(tmpdir(), 'runner-client-'));
  try {
    const addon = join(dir, 'old.cjs');
    writeFileSync(addon, 'exports.load = () => ({});');
    const result = invoke(addon);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, '');
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('report errors from a loaded addon are not swallowed', () => {
  const dir = mkdtempSync(join(tmpdir(), 'runner-client-'));
  try {
    const addon = join(dir, 'broken.cjs');
    writeFileSync(
      addon,
      'exports.load = () => ({ reportUnchanged() { throw Error("report rejected"); } });',
    );
    const result = invoke(addon);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /report rejected/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
