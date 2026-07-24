#!/usr/bin/env node

// Support script for the Fspy Benchmark workflow (.github/workflows/fspy-benchmark.yml).
// Executed directly with `node`, which strips the type annotations at load time,
// so only erasable TypeScript syntax is allowed (no enums, namespaces, etc.).

import { appendFile, mkdir, readFile, writeFile } from 'node:fs/promises';
import { arch, cpus, platform as osPlatform, release } from 'node:os';
import { dirname, join } from 'node:path';
import { pathToFileURL } from 'node:url';

// Mirrors THREAD_COUNT, FILES_PER_THREAD, and PASSES in
// crates/fspy_benchmark/benches/fspy.rs. Only used to describe the workload in
// reports.
const THREAD_COUNT = 4;
const FILES_PER_THREAD = 2048;
const PASSES = 16;

// Bump when the workload or result shape changes; results from different
// schema versions are not comparable.
const SCHEMA_VERSION = 2;

type Args = Map<string, string>;

/** Mean with its confidence interval, in nanoseconds. */
interface EstimateSummary {
  meanNs: number;
  meanLowerNs: number;
  meanUpperNs: number;
}

/** One Criterion estimate, in nanoseconds. */
interface Estimate extends EstimateSummary {
  medianNs: number;
}

/** The subset of a stored benchmark result that reports are rendered from. */
interface ReportInput {
  schemaVersion: number;
  platform: string;
  architecture: string;
  os: { cpu: string };
  commit: string;
  runId: string;
  workload: { threadCount: number; filesPerThread: number; passes: number; totalOpens: number };
  benchmarks: Record<string, EstimateSummary>;
}

/** The full result JSON produced by `collect` and uploaded as an artifact. */
interface BenchmarkResult extends ReportInput {
  os: { platform: string; release: string; cpu: string };
  runner: string;
  runAttempt: string;
  event: string;
  pullRequest: string;
  benchmarks: Record<string, Estimate>;
}

interface BaselineArtifact {
  name: string;
  runId: string;
  createdAt: string;
}

interface IssueComment {
  id: number;
  body?: string | null;
  user?: { type?: string } | null;
}

function parseArgs(args: string[]): Args {
  const parsed = new Map<string, string>();
  for (let index = 0; index < args.length; index += 2) {
    const flag = args[index];
    const value = args[index + 1];
    if (!flag?.startsWith('--') || value === undefined) {
      throw new Error(`invalid arguments near ${flag ?? '<end>'}`);
    }
    parsed.set(flag.slice(2), value);
  }
  return parsed;
}

function required(args: Args, name: string): string {
  const value = args.get(name);
  if (value === undefined || value === '') {
    throw new Error(`missing --${name}`);
  }
  return value;
}

function requiredEnv(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`missing ${name}`);
  return value;
}

async function githubJson<T>(path: string, init: RequestInit = {}): Promise<T> {
  const token = requiredEnv('GITHUB_TOKEN');
  const response = await fetch(`https://api.github.com${path}`, {
    ...init,
    headers: {
      Accept: 'application/vnd.github+json',
      Authorization: `Bearer ${token}`,
      'X-GitHub-Api-Version': '2022-11-28',
      'User-Agent': 'vite-task-fspy-benchmark',
      ...(init.body === undefined ? {} : { 'Content-Type': 'application/json' }),
    },
  });
  if (!response.ok) {
    throw new Error(`GitHub API ${response.status}: ${await response.text()}`);
  }
  return response.json() as Promise<T>;
}

async function findLatestArtifact(
  name: string,
  currentRunId: string,
): Promise<BaselineArtifact | undefined> {
  const repository = requiredEnv('GITHUB_REPOSITORY');
  // The name filter is applied server-side, so one page covers the recent history.
  const response = await githubJson<{
    artifacts: {
      name: string;
      expired: boolean;
      created_at: string;
      workflow_run?: { id: number } | null;
    }[];
  }>(`/repos/${repository}/actions/artifacts?name=${encodeURIComponent(name)}&per_page=100`);
  return (
    response.artifacts
      .flatMap((artifact) =>
        !artifact.expired && artifact.workflow_run?.id != null
          ? [
              {
                name: artifact.name,
                runId: String(artifact.workflow_run.id),
                createdAt: artifact.created_at,
              },
            ]
          : [],
      )
      // Skip the current run so a re-run attempt does not compare against the
      // artifact its own previous attempt uploaded.
      .filter((artifact) => artifact.runId !== currentRunId)
      .sort((left, right) => Date.parse(right.createdAt) - Date.parse(left.createdAt))[0]
  );
}

// Emits step outputs consumed by the workflow's baseline download and result
// upload steps. The baseline is always the latest `main` artifact: comparing
// against earlier runs of the same PR would mask regressions that accumulate
// over the PR's lifetime.
async function resolveBaseline(args: Args): Promise<void> {
  const outputPath = requiredEnv('GITHUB_OUTPUT');
  const platform = required(args, 'platform');
  const context = required(args, 'context');
  const currentRunId = requiredEnv('GITHUB_RUN_ID');
  const currentArtifactName = `fspy-benchmark-${platform}-${context}`;
  const artifact = await findLatestArtifact(`fspy-benchmark-${platform}-main`, currentRunId);

  const outputs = [
    `current-artifact-name=${currentArtifactName}`,
    `found=${artifact ? 'true' : 'false'}`,
  ];
  if (artifact) {
    outputs.push(`artifact-name=${artifact.name}`, `run-id=${artifact.runId}`);
  }
  await appendFile(outputPath, `${outputs.join('\n')}\n`);
}

// Criterion writes the estimates of its most recent run to
// <criterion root>/<target>/<mode>/new/estimates.json.
async function readEstimate(
  criterionRoot: string,
  target: string,
  mode: string,
): Promise<Estimate> {
  const path = join(criterionRoot, target, mode, 'new', 'estimates.json');
  const estimates = JSON.parse(await readFile(path, 'utf8')) as {
    mean: {
      point_estimate: number;
      confidence_interval: { lower_bound: number; upper_bound: number };
    };
    median: { point_estimate: number };
  };
  return {
    meanNs: estimates.mean.point_estimate,
    meanLowerNs: estimates.mean.confidence_interval.lower_bound,
    meanUpperNs: estimates.mean.confidence_interval.upper_bound,
    medianNs: estimates.median.point_estimate,
  };
}

// Combines Criterion's output with run metadata into the JSON document that is
// uploaded as an artifact and later consumed as a comparison baseline.
async function collect(args: Args): Promise<void> {
  const criterionRoot = required(args, 'criterion-root');
  const output = required(args, 'output');
  const platform = required(args, 'platform');
  const expectStatic = args.get('expect-static') === 'true';
  const benchmarks: Record<string, Estimate> = {
    'dynamic/untracked': await readEstimate(criterionRoot, 'dynamic', 'untracked'),
    'dynamic/tracked': await readEstimate(criterionRoot, 'dynamic', 'tracked'),
  };
  if (expectStatic) {
    benchmarks['static/untracked'] = await readEstimate(criterionRoot, 'static', 'untracked');
    benchmarks['static/tracked'] = await readEstimate(criterionRoot, 'static', 'tracked');
  }

  const result: BenchmarkResult = {
    schemaVersion: SCHEMA_VERSION,
    platform,
    architecture: process.env['RUNNER_ARCH'] ?? arch(),
    os: {
      platform: osPlatform(),
      release: release(),
      cpu: cpus()[0]?.model ?? 'unknown',
    },
    runner: args.get('runner') ?? '',
    commit: required(args, 'commit'),
    runId: required(args, 'run-id'),
    runAttempt: args.get('run-attempt') ?? '1',
    event: required(args, 'event'),
    pullRequest: args.get('pull-request') ?? '',
    workload: {
      threadCount: THREAD_COUNT,
      filesPerThread: FILES_PER_THREAD,
      passes: PASSES,
      totalOpens: THREAD_COUNT * FILES_PER_THREAD * PASSES,
    },
    benchmarks,
  };

  await mkdir(dirname(output), { recursive: true });
  await writeFile(output, `${JSON.stringify(result, null, 2)}\n`);
}

/** Wall-clock cost of tracking, as tracked time over untracked time. */
function ratio(result: ReportInput, target: string): number {
  const tracked = result.benchmarks[`${target}/tracked`];
  const untracked = result.benchmarks[`${target}/untracked`];
  if (!tracked || !untracked) throw new Error(`missing benchmark data for ${target}`);
  return tracked.meanNs / untracked.meanNs;
}

function percentage(value: number): string {
  const sign = value > 0 ? '+' : '';
  return `${sign}${(value * 100).toFixed(2)}%`;
}

function duration(nanoseconds: number): string {
  if (nanoseconds >= 1e9) return `${(nanoseconds / 1e9).toFixed(3)} s`;
  if (nanoseconds >= 1e6) return `${(nanoseconds / 1e6).toFixed(3)} ms`;
  if (nanoseconds >= 1e3) return `${(nanoseconds / 1e3).toFixed(3)} µs`;
  return `${nanoseconds.toFixed(1)} ns`;
}

/** Mean with the half-width of its confidence interval, e.g. `12.3 ms ±1.4%`. */
function durationWithUncertainty(estimate: EstimateSummary): string {
  const halfWidth = (estimate.meanUpperNs - estimate.meanLowerNs) / 2 / estimate.meanNs;
  return `${duration(estimate.meanNs)} ±${(halfWidth * 100).toFixed(1)}%`;
}

function resultLink(result: ReportInput): string {
  const repository = process.env['GITHUB_REPOSITORY'];
  if (!repository || !result.runId) return '';
  return `https://github.com/${repository}/actions/runs/${result.runId}`;
}

export function renderReport(current: ReportInput, baseline?: ReportInput): string {
  // Overhead ratios are only comparable within one architecture and one
  // workload: a runner profile can move to different hardware over time, and
  // schema bumps change what is measured.
  const compatible =
    baseline !== undefined &&
    current.schemaVersion === baseline.schemaVersion &&
    current.platform === baseline.platform &&
    current.architecture === baseline.architecture;
  const lines = [`### ${current.platform.charAt(0).toUpperCase()}${current.platform.slice(1)}`, ''];

  if (compatible) {
    const link = resultLink(baseline);
    const description = link ? `[main](${link})` : 'main';
    lines.push(`Compared with ${description} at \`${baseline.commit.slice(0, 8)}\`.`, '');
  } else if (baseline) {
    lines.push(
      baseline.schemaVersion === current.schemaVersion
        ? `No comparison: the baseline architecture is \`${baseline.architecture}\`, current is \`${current.architecture}\`.`
        : `No comparison: the baseline uses result schema ${baseline.schemaVersion}, current is ${current.schemaVersion}.`,
      '',
    );
  } else {
    lines.push('No `main` result was available for this runner.', '');
  }

  lines.push(
    '| Target | Untracked | Tracked | fspy overhead | Normalized change |',
    '| --- | ---: | ---: | ---: | ---: |',
  );

  const targets = Object.hasOwn(current.benchmarks, 'static/tracked')
    ? ['dynamic', 'static']
    : ['dynamic'];
  for (const target of targets) {
    const untracked = current.benchmarks[`${target}/untracked`];
    const tracked = current.benchmarks[`${target}/tracked`];
    if (!untracked || !tracked) throw new Error(`missing benchmark data for ${target}`);
    const overhead = ratio(current, target) - 1;
    // Change of the overhead ratio relative to the baseline. Comparing ratios
    // instead of absolute times cancels machine-speed differences between
    // runner instances.
    const normalizedChange =
      compatible && Object.hasOwn(baseline.benchmarks, `${target}/tracked`)
        ? ratio(current, target) / ratio(baseline, target) - 1
        : undefined;
    lines.push(
      `| ${target} | ${durationWithUncertainty(untracked)} | ${durationWithUncertainty(tracked)} | ${percentage(overhead)} | ${normalizedChange === undefined ? '—' : percentage(normalizedChange)} |`,
    );
  }

  lines.push(
    '',
    `Workload: ${current.workload.threadCount} threads × ${current.workload.filesPerThread.toLocaleString('en-US')} files × ${current.workload.passes.toLocaleString('en-US')} passes, ${current.workload.totalOpens.toLocaleString('en-US')} total open-and-close operations.`,
    '',
    `<sub>\`${current.architecture}\` · ${current.os.cpu} · run [${current.runId}](${resultLink(current)})</sub>`,
    '',
  );
  return `${lines.join('\n')}\n`;
}

async function compare(args: Args): Promise<void> {
  const current = JSON.parse(await readFile(required(args, 'current'), 'utf8')) as BenchmarkResult;
  const baselinePath = args.get('baseline');
  let baseline: BenchmarkResult | undefined;
  if (baselinePath) {
    try {
      baseline = JSON.parse(await readFile(baselinePath, 'utf8')) as BenchmarkResult;
    } catch (error) {
      // The workflow always passes --baseline; the file is absent when no
      // baseline artifact was found.
      if ((error as { code?: string }).code !== 'ENOENT') throw error;
    }
  }
  const report = renderReport(current, baseline);
  await writeFile(required(args, 'output'), report);
  process.stdout.write(report);
}

const COMMENT_MARKER = '<!-- fspy-benchmark-report -->';

// Creates or updates the sticky PR comment. The invisible marker identifies
// the comment, so later runs edit it in place instead of posting a new one.
async function comment(args: Args): Promise<void> {
  const repository = requiredEnv('GITHUB_REPOSITORY');
  const pullRequest = required(args, 'pull-request');
  const commit = required(args, 'commit');
  const resultsDir = required(args, 'results-dir');

  const sections: string[] = [];
  for (const platform of ['linux', 'macos', 'windows']) {
    sections.push(await readFile(join(resultsDir, `${platform}.md`), 'utf8'));
  }
  const body = [
    COMMENT_MARKER,
    '## fspy benchmark',
    '',
    `Results for commit \`${commit}\`. Each platform is compared with the latest \`main\` result.`,
    '',
    ...sections,
    '<sub>This comment is updated by the Fspy Benchmark workflow.</sub>',
    '',
  ].join('\n');

  const comments = await githubJson<IssueComment[]>(
    `/repos/${repository}/issues/${pullRequest}/comments?per_page=100`,
  );
  const existing = comments.find(
    (existingComment) =>
      existingComment.user?.type === 'Bot' && existingComment.body?.includes(COMMENT_MARKER),
  );
  const path = existing
    ? `/repos/${repository}/issues/comments/${existing.id}`
    : `/repos/${repository}/issues/${pullRequest}/comments`;
  await githubJson(path, {
    method: existing ? 'PATCH' : 'POST',
    body: JSON.stringify({ body }),
  });
}

async function main(): Promise<void> {
  const command = process.argv[2];
  const args = parseArgs(process.argv.slice(3));
  if (command === 'resolve-baseline') {
    await resolveBaseline(args);
  } else if (command === 'collect') {
    await collect(args);
  } else if (command === 'compare') {
    await compare(args);
  } else if (command === 'comment') {
    await comment(args);
  } else {
    throw new Error(`unknown command: ${command ?? '<none>'}`);
  }
}

// Run only when executed directly, so the test file can import renderReport.
const invokedPath = process.argv[1] ? pathToFileURL(process.argv[1]).href : '';
if (import.meta.url === invokedPath) {
  main().catch((error: unknown) => {
    const message = error instanceof Error ? (error.stack ?? error.message) : String(error);
    process.stderr.write(`${message}\n`);
    process.exitCode = 1;
  });
}
