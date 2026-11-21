import { generateKeyPairSync, sign, type KeyObject, type webcrypto } from 'node:crypto';

// A stand-in for GitHub Actions' token service, which issues the OIDC tokens
// that authorize uploads. Its key signs tokens for GitHub's issuer, and the
// backend trusts that key in place of GitHub's.

export const issuer = 'https://token.actions.githubusercontent.com';

/** The repository registered with the backend, whose pushes to `main` may upload. */
export const repository = {
  name: 'owner/repository',
  id: '1',
  ownerId: '1',
  branch: 'refs/heads/main',
};

/**
 * The workflow run that each request token stands for. A step chooses one with
 * `ACTIONS_ID_TOKEN_REQUEST_TOKEN`; the default is a push to `main`.
 */
const runs: Record<string, { event_name: string; ref: string }> = {
  'main-push': { event_name: 'push', ref: repository.branch },
  'pull-request': { event_name: 'pull_request', ref: 'refs/pull/1/merge' },
};

export const defaultRequestToken = 'main-push';

/** The key that the backend trusts for upload tokens. */
export interface TrustedKey {
  /** The key ID in each token's header. */
  kid: string;
  publicKey: KeyObject;
  /** The public key as an entry of GitHub's key set. */
  jwk: webcrypto.JsonWebKey & { kid: string };
}

export interface SigningKey extends TrustedKey {
  /**
   * The token that GitHub would issue for `audience` to the run that
   * `requestToken` stands for, or none for an unknown request token.
   */
  issue(requestToken: string, audience: string): string | undefined;
}

function encode(value: unknown): string {
  return Buffer.from(JSON.stringify(value)).toString('base64url');
}

export function createSigningKey(): SigningKey {
  const { publicKey, privateKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
  const kid = 'test';
  return {
    kid,
    publicKey,
    jwk: { ...publicKey.export({ format: 'jwk' }), kid, alg: 'RS256', use: 'sig' },
    issue(requestToken, audience) {
      const run = runs[requestToken];
      if (run === undefined) return undefined;
      const now = Math.floor(Date.now() / 1000);
      const claims = {
        iss: issuer,
        aud: audience,
        sub: `repo:${repository.name}:ref:${run.ref}`,
        iat: now,
        nbf: now,
        exp: now + 300,
        repository: repository.name,
        repository_id: repository.id,
        repository_owner_id: repository.ownerId,
        repository_visibility: 'public',
        ref: run.ref,
        ref_type: 'branch',
        event_name: run.event_name,
      };
      const unsigned = `${encode({ alg: 'RS256', typ: 'JWT', kid })}.${encode(claims)}`;
      return `${unsigned}.${sign('sha256', Buffer.from(unsigned), privateKey).toString('base64url')}`;
    },
  };
}
