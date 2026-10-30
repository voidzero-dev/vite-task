// Checks that each path nf3 links is a symbolic link into `.nf3`, then prints
// the `tslib` version each traced package resolves to through the links.
const fs = require('node:fs');
const path = require('node:path');

const modules = path.resolve('.output/server/node_modules');
for (const link of ['tslib', 'dep-a/node_modules/tslib', 'dep-b/node_modules/tslib']) {
  const linkPath = path.join(modules, link);
  const kind = fs.lstatSync(linkPath).isSymbolicLink() ? 'link' : 'not a link';
  // Resolve both ends, so the output is the same whether the target is
  // relative (Unix) or an absolute junction target (Windows).
  const target = path.relative(fs.realpathSync(path.dirname(linkPath)), fs.realpathSync(linkPath));
  console.log(`${link}: ${kind} to ${target.split(path.sep).join('/')}`);
}
for (const name of ['dep-a', 'dep-b']) {
  console.log(`${name}: tslib ${require(path.join(modules, name))()}`);
}
console.log(`hoisted: tslib ${require(path.join(modules, 'tslib')).version}`);
