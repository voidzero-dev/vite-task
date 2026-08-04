import { expect, test } from 'vitest';
import { page, server } from 'vitest/browser';
import { greeting } from './greeting.js';

test(greeting, async () => {
  document.body.innerHTML = `<h1>${greeting}</h1>`;

  await expect.element(page.getByRole('heading')).toHaveTextContent(greeting);
  expect(server.browser).toBe('chromium');
  expect(server.provider).toBe('playwright');
});
