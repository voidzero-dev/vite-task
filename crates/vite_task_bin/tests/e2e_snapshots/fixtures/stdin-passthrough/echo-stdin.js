// Use writeSync to avoid buffering issues
const fs = require('fs');

// First, verify we're running in a TTY (should print "true" when using expectrl PTY)
fs.writeSync(1, 'TTY: ' + String(process.stdin.isTTY) + '\n');

// Signal the test runner to write "hello from stdin" to our stdin
fs.writeSync(1, '[write-stdin:hello from stdin]');

// Signal that we're done with stdin commands (this triggers EOF handling)
fs.writeSync(1, '[write-stdin:]');

// Read stdin asynchronously - the test runner will send data then EOF
process.stdin.setEncoding('utf8');
process.stdin.once('readable', () => {
  const chunk = process.stdin.read();
  if (chunk !== null) {
    fs.writeSync(1, chunk);
  }
  fs.writeSync(1, 'Done\n');
  process.exit(0);
});
