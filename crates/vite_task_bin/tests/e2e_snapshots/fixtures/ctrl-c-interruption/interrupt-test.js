// Handle SIGINT gracefully
process.on('SIGINT', () => {
  console.log('Received SIGINT, exiting gracefully');
  process.exit(0);
});

// Print magic string to trigger Ctrl+C
console.log('[send-me-ctrl-c]');

// Keep the process alive to receive SIGINT
// (process will be interrupted by the test infrastructure)
setInterval(() => {}, 1000);
