// Tiny Node script that turns on the v22 compile cache and imports a
// few sibling modules so the cache directory actually gets some files
// written to it. On a normally-configured machine the cache lives in
// the OS temp directory (outside the workspace), so the runner doesn't
// see those files when it decides whether the run can be cached.
//
// If the spawned process doesn't have LOCALAPPDATA (or TMP/TEMP/
// USERPROFILE) set on Windows, Node ends up putting the cache inside
// the workspace, the same files are both written and read in this one
// run, and the runner refuses to cache it. That's the bug this fixture
// catches.
import { enableCompileCache } from 'node:module';

enableCompileCache();

await import('./a.mjs');
await import('./b.mjs');
await import('./c.mjs');

console.log('done');
