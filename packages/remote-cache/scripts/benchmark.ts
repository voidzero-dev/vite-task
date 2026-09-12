import assert from 'node:assert/strict';
import { writeFile } from 'node:fs/promises';
import { URL } from 'node:url';
import { arch, platform, cpus } from 'node:os';
import { harness, bytes } from '../test/helpers.ts';
import { decodeEnvelope, encodeEnvelope } from '../src/cbor.ts';
import { defaults, MiB } from '../src/limits.ts';

// Profile only the application isolate, not the Node client or D1/R2 emulators.
class Inspector {
  private id = 0;
  private pending = new Map<
    number,
    { resolve: (value: Record<string, unknown>) => void; reject: (error: Error) => void }
  >();
  constructor(private socket: WebSocket) {
    socket.addEventListener('message', (event) => {
      const response = JSON.parse(String(event.data));
      const request = this.pending.get(response.id);
      if (!request) return;
      this.pending.delete(response.id);
      if (response.error) request.reject(new Error(response.error.message));
      else request.resolve(response.result);
    });
  }
  async call(method: string, params = {}): Promise<Record<string, unknown>> {
    const id = ++this.id;
    const response = new Promise<Record<string, unknown>>((resolve, reject) =>
      this.pending.set(id, { resolve, reject }),
    );
    this.socket.send(JSON.stringify({ id, method, params }));
    return response;
  }
  close() {
    this.socket.close();
  }
}

function sampledCpu(profile: Record<string, unknown>): number {
  const nodes = profile['nodes'] as { id: number; callFrame: { functionName: string } }[];
  const samples = profile['samples'] as number[];
  const deltas = profile['timeDeltas'] as number[];
  const excluded = new Set(
    nodes
      .filter((node) => ['(idle)', '(root)'].includes(node.callFrame.functionName))
      .map((node) => node.id),
  );
  return (
    samples.reduce((sum, node, i) => sum + (excluded.has(node) ? 0 : (deltas[i] ?? 0)), 0) / 1000
  );
}

const h = await harness({ inspector: true });
let inspector: Inspector | undefined;
const measurements: Record<string, unknown>[] = [];
try {
  const address = await h.mf.getInspectorURL();
  const listing = new URL('/json', address.href);
  listing.protocol = 'http:';
  const targets = (await (await fetch(listing)).json()) as {
    id: string;
    webSocketDebuggerUrl: string;
    title: string;
  }[];
  const target =
    targets.find((target) => target.id.includes('core:user:')) ??
    targets.find((target) => !target.id.includes('core:'));
  if (!target) throw new Error('Cannot find application isolate in inspector targets');
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise<void>((resolve, reject) => {
    socket.addEventListener('open', () => resolve(), { once: true });
    socket.addEventListener('error', () => reject(new Error('Inspector connection failed')), {
      once: true,
    });
  });
  inspector = new Inspector(socket);
  await inspector.call('Profiler.enable');
  await inspector.call('Profiler.setSamplingInterval', { interval: 100 });
  const token = await h.token();
  async function measure(name: string, action: () => Promise<void>) {
    await inspector!.call('Profiler.start');
    const start = performance.now();
    await action();
    const wall = performance.now() - start;
    const profile = (await inspector!.call('Profiler.stop'))['profile'] as Record<string, unknown>;
    const heap = await inspector!.call('Runtime.getHeapUsage');
    const row = {
      name,
      wall_ms: Math.round(wall * 100) / 100,
      sampled_cpu_ms: sampledCpu(profile),
      heap_bytes: heap['usedSize'],
      backing_storage_bytes: heap['backingStorageSize'],
    };
    measurements.push(row);
    console.log(JSON.stringify(row));
  }
  for (const mode of ['cold-jwks', 'warm-jwks'])
    await measure(mode, async () => {
      const response = await h.store(bytes(mode), bytes(mode), new Uint8Array(250000), undefined, {
        token,
      });
      assert.equal(response.status, 200, await response.clone().text());
      await response.arrayBuffer();
    });
  for (const valueSize of [250000, defaults.value]) {
    // Codec timings exclude JWT, blob size, and transport. Node CPU is labelled
    // separately; it must not be interpreted as Cloudflare's billed Worker CPU.
    const value = new Uint8Array(valueSize);
    const envelope = encodeEnvelope({ key: bytes('codec'), secondary_key: bytes('codec'), value });
    for (const operation of ['decode', 'encode']) {
      const start = process.cpuUsage();
      for (let i = 0; i < 100; i++) {
        if (operation === 'decode') decodeEnvelope(envelope, true, defaults);
        else encodeEnvelope({ kind: 'exact', value, blob_id: null });
      }
      const cpu = process.cpuUsage(start);
      measurements.push({
        name: `cbor-${operation}-${valueSize}`,
        node_cpu_ms_per_operation: (cpu.user + cpu.system) / 100000,
      });
    }
    await measure(`fetch-value-${valueSize}`, async () => {
      assert.equal(
        (await h.store(bytes('fetch'), bytes('fetch'), value, undefined, { token })).status,
        200,
      );
    });
    await measure(`fetch-response-${valueSize}`, async () => {
      const response = await h.fetch(bytes('fetch'), bytes('fetch'));
      assert.equal(response.status, 200);
      await response.arrayBuffer();
    });
  }
  for (const blobSize of [5_000_000, 20_000_000, 50_000_000, 64 * MiB]) {
    for (const concurrency of [1, 2])
      await measure(`store-${blobSize}-concurrency-${concurrency}`, async () => {
        const result = await Promise.all(
          Array.from({ length: concurrency }, (_, i) =>
            h.store(
              bytes(`size-${blobSize}-${i}`),
              bytes(`size-${blobSize}-${i}`),
              new Uint8Array(defaults.value),
              new Uint8Array(blobSize),
              { token, blobFirst: i % 2 === 0 },
            ),
          ),
        );
        for (const response of result) {
          assert.equal(response.status, 200, await response.clone().text());
          await response.arrayBuffer();
        }
      });
  }
  const result = {
    measured_at: new Date().toISOString(),
    platform: platform(),
    arch: arch(),
    cpu: cpus()[0]?.model,
    node: process.version,
    runtime: 'workerd 1.20260911.1',
    concurrency: { stores: 2, metadata_reads: 4 },
    free_cpu_verified: false,
    caveat:
      'Local V8 sampling is diagnostic. It excludes some native work and is not provider CPU billing. Validate production CPU before enabling a Free release profile.',
    measurements,
  };
  await writeFile(
    new URL('../benchmark-results.json', import.meta.url),
    JSON.stringify(result, null, 2) + '\n',
  );
} finally {
  inspector?.close();
  await h.close();
}
