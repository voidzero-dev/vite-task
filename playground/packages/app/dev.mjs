process.on('SIGINT', () => {
  console.log('app: dev server stopped');
  process.exit(0);
});

setInterval(() => console.log('app: waiting for changes...'), 2000);
