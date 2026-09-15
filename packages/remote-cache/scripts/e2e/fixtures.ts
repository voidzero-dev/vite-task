import { randomUUID } from 'node:crypto';

export type Row = Record<string, unknown>;
export interface Admin {
  sql(sql: string, params?: (string | number | null)[]): Promise<Row[]>;
  put(key: string, value: Uint8Array): Promise<void>;
  delete(key: string): Promise<void>;
  exists(key: string): Promise<boolean>;
}

export const bytes = (value: string) => new TextEncoder().encode(value);
// The REST fixture loader uses generated hexadecimal SQL literals for binary fields.
// This conversion accepts bytes only, never SQL or HTTP input.
export const sqlBytes = (value: Uint8Array) => `X'${Buffer.from(value).toString('hex')}'`;

export interface Fixture {
  scope: string;
  generation: string;
  key: Uint8Array;
  secondary: Uint8Array;
  value: Uint8Array;
  blob: Uint8Array | undefined;
  blobId: string | null;
  valueObject: string;
  blobObject: string;
}

export async function seed(
  admin: Admin,
  scope: string,
  label: string,
  value: Uint8Array,
  blob?: Uint8Array,
): Promise<Fixture> {
  const generation = randomUUID();
  const key = bytes(label),
    secondary = bytes(`${label}-secondary`);
  const valueObject = `${scope}/${generation}/value`,
    blobObject = `${scope}/${generation}/blob`;
  const blobId = blob === undefined ? null : randomUUID();
  await admin.sql(
    `INSERT INTO generations
    (generation_id, scope_id, state, policy_version, token_exp, lease_until, gc_after,
      value_object, blob_object, charged_bytes)
    SELECT ?, scope_id, 'uploading', policy_version, unixepoch() + 900, unixepoch() + 900,
      unixepoch() + 1500, ?, ?, ? FROM scopes WHERE scope_id = ?`,
    [generation, valueObject, blobObject, value.length + (blob?.length ?? 0), scope],
  );
  await admin.put(valueObject, value);
  if (blob !== undefined) await admin.put(blobObject, blob);
  // Use the production publication trigger; no test endpoint or signing-key bypass exists.
  await admin.sql(
    `UPDATE generations SET state = 'ready', key = ${sqlBytes(key)},
    secondary_key = ${sqlBytes(secondary)}, blob_id = ?, value_size = ?, blob_size = ?,
    expires_at = unixepoch() + 86400, gc_after = unixepoch() + 86400
    WHERE generation_id = ?`,
    [blobId, value.length, blob?.length ?? 0, generation],
  );
  return { scope, generation, key, secondary, value, blob, blobId, valueObject, blobObject };
}

export async function retireTestData(admin: Admin): Promise<void> {
  await admin.sql(`UPDATE generations SET expires_at = 0,
    lease_until = min(lease_until, unixepoch()), gc_after = min(gc_after, unixepoch() + 600)
    WHERE scope_id IN ('e2e', 'other')`);
}

export function seedManual(admin: Admin, deployment: string): Promise<Fixture> {
  return seed(
    admin,
    'manual',
    'manual-verification',
    bytes(`deployment:${deployment}`),
    bytes('Public remote cache verification blob\n'),
  );
}
