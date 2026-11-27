// Exercise Deno.execPath() (/proc/self/exe virtualization) by spawning a child
// Deno process directly via Deno.execPath(), plus a Web Worker thread reading a
// second input file — completely offline without downloading denort.
Deno.mkdirSync('dist', { recursive: true });

const childOutPath = 'dist/child.txt';
const child = new Deno.Command(Deno.execPath(), {
  args: [
    'run',
    '--quiet',
    '--no-remote',
    '--allow-read',
    '--allow-write',
    'src/child.ts',
    'src/child-data.txt',
    childOutPath,
  ],
  stdout: 'inherit',
  stderr: 'inherit',
});
const status = child.outputSync();
if (!status.success) {
  Deno.exit(status.code || 1);
}
const childValue = Deno.readTextFileSync(childOutPath).trim();
Deno.removeSync(childOutPath);

const workerValue = await new Promise<string>((resolve, reject) => {
  const worker = new Worker(new URL('./worker.ts', import.meta.url).href, { type: 'module' });
  worker.onmessage = (event: MessageEvent<string>) => resolve(event.data);
  worker.onerror = (event) => reject(event.error ?? new Error(event.message));
  worker.postMessage('src/worker-data.txt');
});

Deno.writeTextFileSync('dist/out.txt', `child=${childValue};worker=${workerValue}\n`);
