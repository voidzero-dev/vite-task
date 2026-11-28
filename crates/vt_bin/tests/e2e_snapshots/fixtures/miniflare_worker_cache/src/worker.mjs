import { payload } from './data.mjs';

export default {
  fetch() {
    return new Response(`worker:${payload}`);
  },
};
