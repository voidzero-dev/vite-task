import { readFileSync } from 'fs';

const content = readFileSync(process.argv[2]);
process.stdout.write(content);
