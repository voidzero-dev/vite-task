import { reportUnchanged } from '@voidzero-dev/vite-task-client';
reportUnchanged();
reportUnchanged();
console.log('JS command ran');
if (process.argv.includes('--fail')) process.exitCode = 7;
