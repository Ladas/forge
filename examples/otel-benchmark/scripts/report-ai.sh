#!/usr/bin/env bash
set -euo pipefail

RESULTS_DIR="${1:?Usage: report-ai.sh <results-dir>}"

if [ ! -d "${RESULTS_DIR}" ]; then
  echo "Error: ${RESULTS_DIR} does not exist"
  exit 1
fi

export BRANCH=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo "unknown")
export COMMIT=$(git rev-parse --short HEAD 2>/dev/null || echo "unknown")
export RESULTS_DIR

python3 << 'PYTHON_SCRIPT'
import json
import glob
import os
from datetime import datetime

results_dir = os.environ['RESULTS_DIR']
branch = os.environ.get('BRANCH', 'unknown')
commit = os.environ.get('COMMIT', 'unknown')

configs = ['ai-baseline', 'ai-otel-noop', 'ai-otel-full']
stats = {}

for config in configs:
    total_p50 = 0
    total_p99 = 0
    total_rps = 0
    count = 0

    pattern = os.path.join(results_dir, f'{config}-run*.json')
    for path in glob.glob(pattern):
        try:
            with open(path) as f:
                data = json.load(f)
                total_p50 += data['latencies']['50th']
                total_p99 += data['latencies']['99th']
                total_rps += data['throughput']
                count += 1
        except (json.JSONDecodeError, KeyError) as e:
            print(f"Warning: Failed to parse {path}: {e}")
            continue

    if count > 0:
        stats[config] = {
            'p50': int(total_p50 / count / 1000),
            'p99': int(total_p99 / count / 1000),
            'rps': int(total_rps / count),
            'runs': count
        }
    else:
        stats[config] = {'p50': 0, 'p99': 0, 'rps': 0, 'runs': 0}

report_path = os.path.join(results_dir, 'report.md')
with open(report_path, 'w') as f:
    f.write(f"# Praxis AI OTel Overhead Benchmark\n\n")
    f.write(f"**Date:** {datetime.now().strftime('%Y-%m-%d')} | **Commit:** {commit} | **Branch:** {branch}\n")
    f.write(f"**Platform:** KIND (1 node) | **Backend:** inference-sim (mock LLM)\n\n")
    f.write(f"## Summary\n\n")
    f.write(f"| Config | P50 (us) | P99 (us) | RPS | Runs | P50 delta | P99 delta |\n")
    f.write(f"|--------|----------|----------|-----|------|-----------|-----------|\n")

    base_p50 = stats['ai-baseline']['p50']
    base_p99 = stats['ai-baseline']['p99']

    for config in configs:
        s = stats[config]
        p50 = s['p50']
        p99 = s['p99']
        rps = s['rps']
        runs = s['runs']

        if config == 'ai-baseline':
            f.write(f"| AI Baseline | {p50} | {p99} | {rps} | {runs} | -- | -- |\n")
        else:
            label = 'AI OTel (noop)' if config == 'ai-otel-noop' else 'AI OTel (full)'

            if base_p50 > 0:
                dp50 = f"{(p50 - base_p50) / base_p50 * 100:+.1f}%"
                dp99 = f"{(p99 - base_p99) / base_p99 * 100:+.1f}%"
            else:
                dp50 = "N/A"
                dp99 = "N/A"

            f.write(f"| {label} | {p50} | {p99} | {rps} | {runs} | {dp50} | {dp99} |\n")

print(f"Report written to: {report_path}")
PYTHON_SCRIPT

cat "${RESULTS_DIR}/report.md"
