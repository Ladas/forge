#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
KIND_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

CLUSTER_NAME="${CLUSTER_NAME:-otel-bench-local}"
CTX="kind-${CLUSTER_NAME}"
GATEWAY_URL="http://localhost:18080"

RATE="${RATE:-2000}"
DURATION="${DURATION:-30s}"
RUNS="${RUNS:-3}"

RESULTS_DIR="${KIND_DIR}/results/$(date +%Y%m%d-%H%M%S)"
mkdir -p "${RESULTS_DIR}"

echo "=== Praxis OTel Benchmark ==="
echo "Rate: ${RATE} RPS | Duration: ${DURATION} | Runs: ${RUNS}"
echo "Results: ${RESULTS_DIR}"
echo ""

run_vegeta() {
  local label="$1"
  local run="$2"
  echo "--- ${label} run ${run}/${RUNS} ---"
  echo "GET ${GATEWAY_URL}/" | \
    vegeta attack -rate="${RATE}" -duration="${DURATION}" -connections=200 | \
    tee "${RESULTS_DIR}/${label}-run${run}.bin" | \
    vegeta report -type=json > "${RESULTS_DIR}/${label}-run${run}.json"
  vegeta report < "${RESULTS_DIR}/${label}-run${run}.bin"
  # Capture resource snapshot
  kubectl --context "${CTX}" top pod -n default --no-headers 2>/dev/null \
    >> "${RESULTS_DIR}/${label}-resources.txt" || true
  echo ""
}

# ---- Run A: Baseline (no OTel) ----
echo "=========================================="
echo "  Run A: Baseline (praxis:dev, no OTel)"
echo "=========================================="

kubectl --context "${CTX}" create configmap praxis-config \
  --from-file=config.yaml="${KIND_DIR}/configs/baseline.yaml" \
  -n default --dry-run=client -o yaml | kubectl --context "${CTX}" apply -f -
kubectl --context "${CTX}" set image deployment/praxis-proxy praxis-proxy=praxis:dev -n default
kubectl --context "${CTX}" set env deployment/praxis-proxy OTEL_EXPORTER_OTLP_ENDPOINT- -n default 2>/dev/null || true
kubectl --context "${CTX}" scale deployment/praxis-proxy --replicas=0 -n default
sleep 3
kubectl --context "${CTX}" scale deployment/praxis-proxy --replicas=1 -n default
kubectl --context "${CTX}" -n default wait --for=condition=Available deployment/praxis-proxy --timeout 60s
sleep 5

echo "Warmup..."
echo "GET ${GATEWAY_URL}/" | vegeta attack -rate=500 -duration=10s > /dev/null 2>&1 || true
sleep 2

for i in $(seq 1 "${RUNS}"); do
  run_vegeta "baseline" "${i}"
  sleep 5
done

# ---- Run B: OTel noop (spans created, not exported) ----
echo "=========================================="
echo "  Run B: OTel noop (praxis:dev-otel, no endpoint)"
echo "=========================================="

kubectl --context "${CTX}" create configmap praxis-config \
  --from-file=config.yaml="${KIND_DIR}/configs/otel-noop.yaml" \
  -n default --dry-run=client -o yaml | kubectl --context "${CTX}" apply -f -
kubectl --context "${CTX}" set image deployment/praxis-proxy praxis-proxy=praxis:dev-otel -n default
kubectl --context "${CTX}" set env deployment/praxis-proxy OTEL_EXPORTER_OTLP_ENDPOINT- -n default 2>/dev/null || true
kubectl --context "${CTX}" scale deployment/praxis-proxy --replicas=0 -n default
sleep 3
kubectl --context "${CTX}" scale deployment/praxis-proxy --replicas=1 -n default
kubectl --context "${CTX}" -n default wait --for=condition=Available deployment/praxis-proxy --timeout 60s
sleep 5

echo "Warmup..."
echo "GET ${GATEWAY_URL}/" | vegeta attack -rate=500 -duration=10s > /dev/null 2>&1 || true
sleep 2

for i in $(seq 1 "${RUNS}"); do
  run_vegeta "otel-noop" "${i}"
  sleep 5
done

# ---- Run C: OTel full (spans exported to collector) ----
echo "=========================================="
echo "  Run C: OTel full (praxis:dev-otel, exporting)"
echo "=========================================="

# Set OTEL env var BEFORE restarting the pod
kubectl --context "${CTX}" create configmap praxis-config \
  --from-file=config.yaml="${KIND_DIR}/configs/otel-full.yaml" \
  -n default --dry-run=client -o yaml | kubectl --context "${CTX}" apply -f -
kubectl --context "${CTX}" set image deployment/praxis-proxy praxis-proxy=praxis:dev-otel -n default
kubectl --context "${CTX}" set env deployment/praxis-proxy \
  OTEL_EXPORTER_OTLP_ENDPOINT=http://otel-collector.otel.svc:4317 -n default
kubectl --context "${CTX}" scale deployment/praxis-proxy --replicas=0 -n default
sleep 3
kubectl --context "${CTX}" scale deployment/praxis-proxy --replicas=1 -n default
kubectl --context "${CTX}" -n default wait --for=condition=Available deployment/praxis-proxy --timeout 60s
sleep 5

echo "Warmup..."
echo "GET ${GATEWAY_URL}/" | vegeta attack -rate=500 -duration=10s > /dev/null 2>&1 || true
sleep 2

for i in $(seq 1 "${RUNS}"); do
  run_vegeta "otel-full" "${i}"
  sleep 5
done

echo "=========================================="
echo "  Benchmark complete"
echo "=========================================="
echo "Results in: ${RESULTS_DIR}"
echo ""
echo "Generate report:"
echo "  bash ${SCRIPT_DIR}/report.sh ${RESULTS_DIR}"
