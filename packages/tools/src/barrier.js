#!/usr/bin/env node

// barrier <dir> <prefix> <count> [--exit=<code>] [--hang]
//
// Cross-platform concurrency barrier for testing.
// Creates <dir>/<prefix>_<pid>, then waits (via fs.watch) for <count> files
// matching <prefix>_* to exist in <dir>.
//
// Options:
//   --exit=<code>  Exit with the given code after the barrier is met.
//   --hang         Block on stdin after the barrier is met (for kill tests).
//
// If tasks run concurrently, all participants arrive and the barrier resolves.
// If tasks run sequentially, the first participant waits forever (test timeout).

import fs from 'node:fs';
import path from 'node:path';

const positional = [];
let exitCode = 0;
let hang = false;

for (const arg of process.argv.slice(2)) {
  if (arg.startsWith('--exit=')) {
    exitCode = parseInt(arg.slice(7), 10);
  } else if (arg === '--hang') {
    hang = true;
  } else {
    positional.push(arg);
  }
}

const [dir, prefix, countStr] = positional;
const count = parseInt(countStr, 10);

fs.mkdirSync(dir, { recursive: true });

// Create this participant's marker file.
const markerName = `${prefix}_${process.pid}`;
fs.writeFileSync(path.join(dir, markerName), '');

function countMatches() {
  return fs.readdirSync(dir).filter((f) => f.startsWith(`${prefix}_`)).length;
}

function onBarrierMet() {
  if (hang) {
    // Keep the process alive indefinitely — killed via signal when the runner cancels.
    // Use setInterval rather than stdin.resume() for cross-platform reliability.
    setInterval(() => {}, 1 << 30);
    return;
  }
  process.exit(exitCode);
}

// Start watching before the initial check to avoid missing events
// between the check and the watch setup.
const watcher = fs.watch(dir, () => {
  if (countMatches() >= count) {
    watcher.close();
    onBarrierMet();
  }
});

// Check immediately in case all participants already arrived.
if (countMatches() >= count) {
  watcher.close();
  onBarrierMet();
}
