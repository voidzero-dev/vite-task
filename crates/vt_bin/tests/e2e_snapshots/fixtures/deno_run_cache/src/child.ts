const [input, output] = Deno.args;
const content = Deno.readTextFileSync(input).trim();
Deno.writeTextFileSync(output, content);
