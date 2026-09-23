import { expect, test } from 'vite-plus/test';
import { page, server } from 'vite-plus/test/browser';
import { greeting } from './greeting.js';

test(greeting, async () => {
  document.body.innerHTML = `<h1>${greeting}</h1>`;

  await expect.element(page.getByRole('heading')).toHaveTextContent(greeting);
  expect(server.browser).toBe('chromium');
  expect(server.provider).toBe('playwright');
});
