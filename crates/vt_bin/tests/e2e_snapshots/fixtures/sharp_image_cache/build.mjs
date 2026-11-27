import fs from 'node:fs';
import sharp from 'sharp';

fs.mkdirSync('dist', { recursive: true });
await sharp('src/icon.svg').resize(8, 8).png().toFile('dist/out.png');
const { data, info } = await sharp('dist/out.png')
  .ensureAlpha()
  .raw()
  .toBuffer({ resolveWithObject: true });
const pixel = [data[0], data[1], data[2], data[3]].join(',');
fs.writeFileSync('dist/meta.txt', `${info.width}x${info.height}:${pixel}\n`);
