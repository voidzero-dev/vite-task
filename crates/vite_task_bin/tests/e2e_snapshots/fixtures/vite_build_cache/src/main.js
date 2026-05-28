// `import.meta.env.VITE_MODE` is replaced at build time from the value vite
// picks up for keys matching `envPrefix` (`VITE_` by default). The markers
// let the e2e test assert that flipping VITE_MODE actually changed what was
// built and that glob-tracking invalidates the cache.
if (import.meta.env.VITE_MODE === 'production') {
  document.body.append('BUILD_MODE_PROD');
} else {
  document.body.append('BUILD_MODE_DEV');
}
