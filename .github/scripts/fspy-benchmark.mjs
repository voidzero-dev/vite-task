#!/usr/bin/env node

import { appendFile, mkdir, readFile, writeFile } from 'node:fs/promises';
import { arch, cpus, platform as osPlatform, release } from 'node:os';
import { dirname, join } from 'node:path';
import { pathToFileURL } from 'node:url';

const THREAD_COUNT = 4;
const FILES_PER_THREAD = 2048;

function parseArgs(args) {
  const parsed = new Map();
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

function required(args, name) {
  const value = args.get(name);
  if (value === undefined || value === '') {
    throw new Error(`missing --${name}`);
  }
  return value;
}

async function githubJson(path) {
  const token = requiredEnv('GITHUB_TOKEN');
  const response = await fetch(`https://api.github.com${path}`, {
    headers: {
      Accept: 'application/vnd.github+json',
      Authorization: `Bearer ${token}`,
      'X-GitHub-Api-Version': '2022-11-28',
      'User-Agent': 'vite-task-fspy-benchmark',
    },
  });
  if (!response.ok) {
    throw new Error(`GitHub API ${response.status}: ${await response.text()}`);
  }
  return response.json();
}

function requiredEnv(name) {
  const value = process.env[name];
  if (!value) throw new Error(`missing ${name}`);
  return value;
}

async function findLatestArtifact(name, currentRunId) {
  const repository = requiredEnv('GITHUB_REPOSITORY');
  const response = await githubJson(
    `/repos/${repository}/actions/artifacts?name=${encodeURIComponent(name)}&per_page=100`,
  );
  return response.artifacts
    .filter(
      (artifact) => !artifact.expired && String(artifact.workflow_run?.id) !== String(currentRunId),
    )
    .sort((left, right) => Date.parse(right.created_at) - Date.parse(left.created_at))[0];
}

async function resolveBaseline(args) {
  const outputPath = requiredEnv('GITHUB_OUTPUT');
  const platform = required(args, 'platform');
  const context = required(args, 'context');
  const currentRunId = requiredEnv('GITHUB_RUN_ID');
  const currentArtifactName = `fspy-benchmark-${platform}-${context}`;
  const candidates = [currentArtifactName];
  if (context !== 'main') candidates.push(`fspy-benchmark-${platform}-main`);

  let artifact;
  let kind;
  for (const candidate of candidates) {
    artifact = await findLatestArtifact(candidate, currentRunId);
    if (artifact) {
      kind = candidate === currentArtifactName ? 'previous PR run' : 'main';
      break;
    }
  }

  const outputs = [
    `current-artifact-name=${currentArtifactName}`,
    `found=${artifact ? 'true' : 'false'}`,
  ];
  if (artifact) {
    outputs.push(
      `artifact-name=${artifact.name}`,
      `run-id=${artifact.workflow_run.id}`,
      `kind=${kind}`,
    );
  }
  await appendFile(outputPath, `${outputs.join('\n')}\n`);
}

async function readEstimate(criterionRoot, target, mode) {
  const path = join(criterionRoot, target, mode, 'new', 'estimates.json');
  const estimates = JSON.parse(await readFile(path, 'utf8'));
  return {
    meanNs: estimates.mean.point_estimate,
    meanLowerNs: estimates.mean.confidence_interval.lower_bound,
    meanUpperNs: estimates.mean.confidence_interval.upper_bound,
    medianNs: estimates.median.point_estimate,
  };
}

async function collect(args) {
  const criterionRoot = required(args, 'criterion-root');
  const output = required(args, 'output');
  const platform = required(args, 'platform');
  const expectStatic = args.get('expect-static') === 'true';
  const benchmarks = {
    'dynamic/untracked': await readEstimate(criterionRoot, 'dynamic', 'untracked'),
    'dynamic/tracked': await readEstimate(criterionRoot, 'dynamic', 'tracked'),
  };
  if (expectStatic) {
    benchmarks['static/untracked'] = await readEstimate(criterionRoot, 'static', 'untracked');
    benchmarks['static/tracked'] = await readEstimate(criterionRoot, 'static', 'tracked');
  }

  const result = {
    schemaVersion: 1,
    platform,
    architecture: process.env.RUNNER_ARCH ?? arch(),
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
      totalOpens: THREAD_COUNT * FILES_PER_THREAD,
    },
    benchmarks,
  };

  await mkdir(dirname(output), { recursive: true });
  await writeFile(output, `${JSON.stringify(result, null, 2)}\n`);
}

function ratio(result, target) {
  return (
    result.benchmarks[`${target}/tracked`].meanNs / result.benchmarks[`${target}/untracked`].meanNs
  );
}

function percentage(value) {
  const sign = value > 0 ? '+' : '';
  return `${sign}${(value * 100).toFixed(2)}%`;
}

function duration(nanoseconds) {
  if (nanoseconds >= 1e9) return `${(nanoseconds / 1e9).toFixed(3)} s`;
  if (nanoseconds >= 1e6) return `${(nanoseconds / 1e6).toFixed(3)} ms`;
  if (nanoseconds >= 1e3) return `${(nanoseconds / 1e3).toFixed(3)} µs`;
  return `${nanoseconds.toFixed(1)} ns`;
}

function resultLink(result) {
  const repository = process.env.GITHUB_REPOSITORY;
  if (!repository || !result.runId) return '';
  return `https://github.com/${repository}/actions/runs/${result.runId}`;
}

export function renderReport(current, baseline, baselineKind = '') {
  const compatible =
    baseline &&
    current.platform === baseline.platform &&
    current.architecture === baseline.architecture;
  const lines = [`### ${current.platform[0].toUpperCase()}${current.platform.slice(1)}`, ''];

  if (compatible) {
    const link = resultLink(baseline);
    const description = link
      ? `[${baselineKind || 'baseline'}](${link})`
      : baselineKind || 'baseline';
    lines.push(`Compared with ${description} at \`${baseline.commit.slice(0, 8)}\`.`, '');
  } else if (baseline) {
    lines.push(
      `No comparison: the baseline architecture is \`${baseline.architecture}\`, current is \`${current.architecture}\`.`,
      '',
    );
  } else {
    lines.push('No previous result was available for this runner.', '');
  }

  lines.push(
    '| Target | Untracked | Tracked | fspy overhead | Normalized change |',
    '| --- | ---: | ---: | ---: | ---: |',
  );

  const targets = Object.hasOwn(current.benchmarks, 'static/tracked')
    ? ['dynamic', 'static']
    : ['dynamic'];
  for (const target of targets) {
    const untracked = current.benchmarks[`${target}/untracked`].meanNs;
    const tracked = current.benchmarks[`${target}/tracked`].meanNs;
    const overhead = ratio(current, target) - 1;
    const normalizedChange =
      compatible && Object.hasOwn(baseline.benchmarks, `${target}/tracked`)
        ? ratio(current, target) / ratio(baseline, target) - 1
        : undefined;
    lines.push(
      `| ${target} | ${duration(untracked)} | ${duration(tracked)} | ${percentage(overhead)} | ${normalizedChange === undefined ? '—' : percentage(normalizedChange)} |`,
    );
  }

  lines.push(
    '',
    `Workload: ${current.workload.threadCount} threads, ${current.workload.filesPerThread.toLocaleString('en-US')} files per thread, ${current.workload.totalOpens.toLocaleString('en-US')} total open-and-close operations.`,
    '',
    `<sub>\`${current.architecture}\` · ${current.os.cpu} · run [${current.runId}](${resultLink(current)})</sub>`,
    '',
  );
  return `${lines.join('\n')}\n`;
}

async function compare(args) {
  const current = JSON.parse(await readFile(required(args, 'current'), 'utf8'));
  const baselinePath = args.get('baseline');
  let baseline;
  if (baselinePath) {
    try {
      baseline = JSON.parse(await readFile(baselinePath, 'utf8'));
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
    }
  }
  const report = renderReport(current, baseline, args.get('baseline-kind') ?? '');
  await writeFile(required(args, 'output'), report);
  process.stdout.write(report);
}

async function main() {
  const command = process.argv[2];
  const args = parseArgs(process.argv.slice(3));
  if (command === 'resolve-baseline') {
    await resolveBaseline(args);
  } else if (command === 'collect') {
    await collect(args);
  } else if (command === 'compare') {
    await compare(args);
  } else {
    throw new Error(`unknown command: ${command ?? '<none>'}`);
  }
}

const invokedPath = process.argv[1] ? pathToFileURL(process.argv[1]).href : '';
if (import.meta.url === invokedPath) {
  main().catch((error) => {
    process.stderr.write(`${error.stack ?? error}\n`);
    process.exitCode = 1;
  });
}
