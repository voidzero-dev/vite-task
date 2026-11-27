import fs from 'node:fs';
import path from 'node:path';

const [input, output] = process.argv.slice(2);
const content = fs.readFileSync(input, 'utf8').trim();
fs.mkdirSync(path.dirname(output), { recursive: true });
fs.writeFileSync(output, `bun-compiled:${content}\n`);
