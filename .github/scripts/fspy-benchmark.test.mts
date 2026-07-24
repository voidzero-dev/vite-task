import assert from 'node:assert/strict';
import test from 'node:test';

import { renderReport } from './fspy-benchmark.mts';

function estimate(meanNs: number) {
  return { meanNs, meanLowerNs: meanNs * 0.98, meanUpperNs: meanNs * 1.02 };
}

function result(
  platform: string,
  architecture: string,
  runId: string,
  untracked: number,
  tracked: number,
) {
  return {
    schemaVersion: 2,
    platform,
    architecture,
    os: { cpu: 'Test CPU' },
    commit: '1234567890abcdef',
    runId,
    workload: { threadCount: 4, filesPerThread: 2048, passes: 16, totalOpens: 131072 },
    benchmarks: {
      'dynamic/untracked': estimate(untracked),
      'dynamic/tracked': estimate(tracked),
    },
  };
}

void test('renders normalized change against a compatible baseline', () => {
  const baseline = result('linux', 'X64', '10', 100, 120);
  const current = result('linux', 'X64', '20', 100, 132);
  const report = renderReport(current, baseline);

  assert.match(report, /fspy overhead \| Normalized change/);
  assert.match(report, /100\.0 ns ±2\.0% \| 132\.0 ns ±2\.0% \| \+32\.00% \| \+10\.00%/);
  assert.match(report, /Compared with main at `12345678`/);
});

void test('does not compare different architectures', () => {
  const baseline = result('macos', 'X64', '10', 100, 120);
  const current = result('macos', 'ARM64', '20', 100, 132);
  const report = renderReport(current, baseline);

  assert.match(report, /No comparison/);
  assert.match(report, /\+32\.00% \| —/);
});

void test('does not compare different schema versions', () => {
  const baseline = { ...result('linux', 'X64', '10', 100, 120), schemaVersion: 1 };
  const current = result('linux', 'X64', '20', 100, 132);
  const report = renderReport(current, baseline);

  assert.match(report, /No comparison: the baseline uses result schema 1, current is 2\./);
  assert.match(report, /\+32\.00% \| —/);
});
