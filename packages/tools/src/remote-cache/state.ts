import { join } from 'node:path';

/** The directory, relative to the current directory, that holds a case's backend. */
export const stateDirectory = 'remote-cache';

/** The path below the backend's origin that every endpoint has. */
export const basePath = '/projects/test';

/** How a running backend is reached, saved in `server.json`. */
export interface ServerInfo {
  /** The endpoint commands use, e.g. `http://127.0.0.1:1234/projects/test`. */
  url: string;
  /** The origin of the control server, e.g. `http://127.0.0.1:1235`. */
  control: string;
  /** Where to request GitHub Actions OIDC tokens, as `ACTIONS_ID_TOKEN_REQUEST_URL`. */
  tokenRequestUrl: string;
}

export function serverFile(directory: string): string {
  return join(directory, 'server.json');
}

export function logFile(directory: string): string {
  return join(directory, 'server.log');
}
