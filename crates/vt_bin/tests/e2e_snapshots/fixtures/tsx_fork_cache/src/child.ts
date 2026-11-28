import fs from 'node:fs';

const dataPath: string = process.argv[2]!;
const content: string = fs.readFileSync(dataPath, 'utf8').trim();
process.send?.(content, () => {
  process.disconnect();
});
