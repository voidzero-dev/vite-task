// Prints the `tslib` version each traced package resolves to in the server output.
const modules = './.output/server/node_modules/';
for (const name of ['dep-a', 'dep-b']) {
  console.log(`${name}: tslib ${require(modules + name)()}`);
}
console.log(`hoisted: tslib ${require(modules + 'tslib').version}`);
