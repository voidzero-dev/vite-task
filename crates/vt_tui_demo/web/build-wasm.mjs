// Builds `vt_tui_demo` for the browser and generates its JS bindings in
// `src/pkg`, which the page imports.
import { execFileSync } from 'node:child_process';
import { join } from 'node:path';

const run = (command, args) => execFileSync(command, args, { stdio: 'inherit' });
const metadata = JSON.parse(
  execFileSync('cargo', ['metadata', '--format-version', '1', '--no-deps'], { encoding: 'utf8' }),
);

// Only the library, as a cdylib: the crate's binary is for native terminals.
run('cargo', [
  'rustc',
  '-p',
  'vt_tui_demo',
  '--lib',
  '--crate-type',
  'cdylib',
  '--target',
  'wasm32-unknown-unknown',
  '--release',
]);
run('wasm-bindgen', [
  '--target',
  'web',
  '--out-dir',
  join(import.meta.dirname, 'src', 'pkg'),
  join(metadata.target_directory, 'wasm32-unknown-unknown', 'release', 'vt_tui_demo.wasm'),
]);
