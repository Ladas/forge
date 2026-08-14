#!/usr/bin/env bash
set -euo pipefail

RESULTS_DIR="${1:?Usage: report.sh <results-dir>}"

if [ ! -d "${RESULTS_DIR}" ]; then
  echo "Error: ${RESULTS_DIR} does not exist"
  exit 1
fi

BRANCH=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo "unknown")
COMMIT=$(git rev-parse --short HEAD 2>/dev/null || echo "unknown")

REPORT="${RESULTS_DIR}/report.md"

# Use Python to generate the report (macOS has bash 3.2, no associative arrays)
python3 << 'PYTHON_SCRIPT'
import json
import glob
import sys
import os
from datetime import datetime

results_dir = os.environ['RESULTS_DIR']
branch = os.environ['BRANCH']
commit = os.environ['COMMIT']

configs = ['baseline', 'otel-noop', 'otel-full']
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
            print(f"Warning: Failed to parse {path}: {e}", file=sys.stderr)
            continue

    if count > 0:
        stats[config] = {
            'p50': int(total_p50 / count / 1000),  # Convert to microseconds
            'p99': int(total_p99 / count / 1000),
            'rps': int(total_rps / count),
            'runs': count
        }
    else:
        stats[config] = {'p50': 0, 'p99': 0, 'rps': 0, 'runs': 0}

# Generate report
report_path = os.path.join(results_dir, 'report.md')
with open(report_path, 'w') as f:
    f.write(f"# Praxis OTel Overhead Benchmark\n\n")
    f.write(f"**Date:** {datetime.now().strftime('%Y-%m-%d')} | **Commit:** {commit} | **Branch:** {branch}\n")
    f.write(f"**Platform:** KIND (1 node) | **Backend:** Fortio echo\n\n")
    f.write(f"## Summary\n\n")
    f.write(f"| Config | P50 (us) | P99 (us) | RPS | Runs | P50 delta | P99 delta |\n")
    f.write(f"|--------|----------|----------|-----|------|-----------|-----------||\n")

    base_p50 = stats['baseline']['p50']
    base_p99 = stats['baseline']['p99']

    for config in configs:
        s = stats[config]
        p50 = s['p50']
        p99 = s['p99']
        rps = s['rps']
        runs = s['runs']

        if config == 'baseline':
            f.write(f"| Baseline | {p50} | {p99} | {rps} | {runs} | -- | -- |\n")
        else:
            label = 'OTel (noop)' if config == 'otel-noop' else 'OTel (full)'

            if base_p50 > 0:
                dp50 = f"{(p50 - base_p50) / base_p50 * 100:+.1f}%"
                dp99 = f"{(p99 - base_p99) / base_p99 * 100:+.1f}%"
            else:
                dp50 = "N/A"
                dp99 = "N/A"

            f.write(f"| {label} | {p50} | {p99} | {rps} | {runs} | {dp50} | {dp99} |\n")

    f.write(f"\n## Previous Results (reference)\n\n")
    f.write(f"| Config | P50 (us) | P99 (us) | RPS | P50 delta | P99 delta |\n")
    f.write(f"|--------|----------|----------|-----|-----------|-----------||\n")
    f.write(f"| Baseline | 388 | 568 | 2,000 | -- | -- |\n")
    f.write(f"| OTel noop | 386 | 558 | 2,000 | -0.5% | -1.8% |\n")
    f.write(f"| OTel full | 392 | 592 | 2,000 | +1.0% | +4.2% |\n\n")
    f.write(f"Note: Previous results were from the old branch without root spans.\n")

print(f"Report written to: {report_path}")
PYTHON_SCRIPT

echo ""
cat "${REPORT}"
