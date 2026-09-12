import { authorize, githubKeys, type WriteIdentity } from './auth.ts';
import { cborResponse, decodeEnvelope } from './cbor.ts';
import { cleanup, getScope, selectBlob, selectEntry } from './database.ts';
import { badRequest, errorResponse, HttpError, unavailable } from './errors.ts';
import { limitsFrom } from './limits.ts';
import { parameters } from './multipart.ts';
import { store } from './store.ts';
import { contentLength, Deadline, readBody } from './streams.ts';
import { Admission } from './admission.ts';
import { Observations } from './observations.ts';

const keys = githubKeys();
const admission = new Admission();

export default {
  async fetch(request, env, ctx) {
    const started = Date.now();
    const id = crypto.randomUUID();
    const stats = new Observations();
    let scopeId = 'unknown';
    let operation = 'unknown';
    let outcome = 'error';
    let identity: WriteIdentity | undefined;
    let deadline: Deadline | undefined;
    let release: (() => void) | undefined;
    let response: Response;
    try {
      const limits = limitsFrom(env.LIMITS);
      const url = new URL(request.url);
      deadline = new Deadline(
        url.pathname.endsWith('/store') ? limits.deadlineMs : Math.min(limits.deadlineMs, 15000),
        request.signal,
      );
      const route =
        /^\/projects\/([a-z0-9][a-z0-9-]{0,62})\/(fetch|store|blob\/([0-9a-f-]{36}))$/.exec(
          url.pathname,
        );
      const namespaces: unknown = JSON.parse(env.NAMESPACES);
      if (
        !Array.isArray(namespaces) ||
        namespaces.length > 100 ||
        namespaces.some((value) => typeof value !== 'string')
      )
        throw new Error('Invalid namespaces');
      const known = route && namespaces.includes(route[1]);
      if (known) {
        scopeId = route[1]!;
        operation = route[2]!.startsWith('blob/') ? 'blob' : route[2]!;
      }
      const limiter =
        operation === 'store' || url.pathname.endsWith('/store')
          ? env.STORE_LIMITER
          : env.READ_LIMITER;
      if (
        !(await deadline.run(limiter.limit({ key: known ? `${scopeId}:${operation}` : 'unknown' })))
          .success
      )
        throw new HttpError(429, 'rate_limit');
      if (!known || url.search || request.method !== (operation === 'blob' ? 'GET' : 'POST'))
        throw new HttpError(404, 'unknown_route');
      release = admission.acquire(operation === 'store');
      const scope = await deadline.run(getScope(env.INDEX, scopeId, stats));
      if (operation === 'store') {
        identity = await deadline.run(authorize(request, scope, keys));
        response = await store(request, env, ctx, scope, identity, limits, deadline, stats);
        outcome = 'stored';
      } else if (operation === 'fetch') {
        if (parameters(request.headers.get('Content-Type') ?? '').type !== 'application/cbor')
          badRequest();
        contentLength(request, limits.fetch);
        const body = await readBody(request.body, limits.fetch, deadline);
        stats.request_bytes = body.length;
        const data = decodeEnvelope(body, false, limits);
        const selected = await deadline.run(
          selectEntry(env.INDEX, scopeId, data.key, data.secondary_key, stats),
        );
        if (!selected) {
          outcome = 'miss';
          throw new HttpError(404, 'miss');
        }
        stats.r2_operations++;
        const object = await deadline
          .run(env.ARTIFACTS.get(selected.value_object))
          .catch(() => unavailable());
        if (!object || object.size !== selected.value_size || object.size > limits.value)
          unavailable();
        const value = await readBody(object.body, limits.value, deadline).catch(() =>
          unavailable(),
        );
        if (value.length !== selected.value_size) unavailable();
        response = cborResponse({
          kind: selected.kind,
          ...(selected.kind === 'fallback' ? { key: new Uint8Array(selected.key) } : {}),
          value,
          blob_id: selected.blob_id,
        });
        outcome = selected.kind;
      } else {
        const selected = await deadline.run(selectBlob(env.INDEX, scopeId, route[3]!, stats));
        if (!selected) throw new HttpError(404, 'blob_missing');
        stats.r2_operations++;
        const object = await deadline
          .run(env.ARTIFACTS.get(selected.blob_object))
          .catch(() => unavailable());
        if (!object) throw new HttpError(404, 'blob_missing');
        if (object.size !== selected.blob_size) unavailable();
        response = new Response(object.body, {
          headers: {
            'Content-Type': 'application/octet-stream',
            'Content-Length': String(object.size),
            'Cache-Control': 'no-store',
          },
        });
        outcome = 'blob';
      }
    } catch (error) {
      response = errorResponse(error);
      if (error instanceof HttpError) outcome = error.code;
    } finally {
      deadline?.dispose();
      release?.();
    }
    response.headers.set('X-Request-Id', id);
    const sample = Math.min(1, Math.max(0, Number(env.LOG_SAMPLE_RATE) || 0));
    // Fixed sampling bounds error volume as well as successful request volume.
    if (Math.random() < sample)
      console.log(
        JSON.stringify({
          request_id: id,
          scope: scopeId,
          operation,
          status: response.status,
          outcome,
          ...(response.status >= 400 ? { error_class: outcome } : {}),
          response_bytes: Number(response.headers.get('Content-Length') ?? 0),
          duration_ms: Date.now() - started,
          ...stats.toJSON(),
          ...(stats.d1_storage_bytes >= 400000000 ? { warning: 'd1_storage_400mb' } : {}),
          ...(identity
            ? {
                repository_id: identity.repository_id,
                workflow_ref: identity.workflow_ref,
                run_id: identity.run_id,
                run_attempt: identity.run_attempt,
                sha: identity.sha,
              }
            : {}),
        }),
      );
    return response;
  },
  async scheduled(_event, env) {
    await cleanup(env);
  },
} satisfies ExportedHandler<Env>;
