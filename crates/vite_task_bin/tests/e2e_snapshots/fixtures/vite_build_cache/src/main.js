// Vite replaces this at build time from keys matching `envPrefix` (`VITE_` by
// default), so the e2e can verify that glob-tracked env changes rebuild output.
console.log(import.meta.env.VITE_CACHE_LABEL);
