import fs from 'node:fs';
import * as sass from 'sass-embedded';

const result = sass.compile('src/main.scss');
fs.mkdirSync('dist', { recursive: true });
fs.writeFileSync('dist/main.css', `${result.css}\n`);
