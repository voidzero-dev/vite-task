self.onmessage = (event: MessageEvent<string>) => {
  const content = Deno.readTextFileSync(event.data).trim();
  self.postMessage(content);
  self.close();
};
