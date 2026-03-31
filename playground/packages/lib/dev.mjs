process.on('SIGINT', () => {
  console.log('lib: dev server stopped');
  process.exit(0);
});

setInterval(() => console.log('lib: waiting for changes...'), 2000);
