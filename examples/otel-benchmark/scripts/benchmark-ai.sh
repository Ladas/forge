#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
KIND_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

CLUSTER_NAME="${CLUSTER_NAME:-otel-bench-local}"
CTX="kind-${CLUSTER_NAME}"
GATEWAY_URL="http://localhost:18080"

RATE="${RATE:-500}"
DURATION="${DURATION:-30s}"
RUNS="${RUNS:-3}"

RESULTS_DIR="${KIND_DIR}/results/ai-$(date +%Y%m%d-%H%M%S)"
mkdir -p "${RESULTS_DIR}"

echo "=== Praxis AI OTel Benchmark ==="
echo "Rate: ${RATE} RPS | Duration: ${DURATION} | Runs: ${RUNS}"
echo "Results: ${RESULTS_DIR}"
echo ""

run_vegeta() {
  local label="$1"
  local run="$2"
  echo "--- ${label} run ${run}/${RUNS} ---"
  printf 'POST %s/v1/chat/completions\nContent-Type: application/json\n@%s\n' \
    "${GATEWAY_URL}" "${SCRIPT_DIR}/ai-payload.json" | \
    vegeta attack -rate="${RATE}" -duration="${DURATION}" -connections=100 | \
    tee "${RESULTS_DIR}/${label}-run${run}.bin" | \
    vegeta report -type=json > "${RESULTS_DIR}/${label}-run${run}.json"
  vegeta report < "${RESULTS_DIR}/${label}-run${run}.bin"
  kubectl --context "${CTX}" top pod -n default --no-headers 2>/dev/null \
    >> "${RESULTS_DIR}/${label}-resources.txt" || true
  echo ""
}

# ---- Run A: AI Baseline (no OTel) ----
echo "=========================================="
echo "  Run A: AI Baseline (praxis-ai:dev, no OTel)"
echo "=========================================="

kubectl --context "${CTX}" create configmap praxis-config \
  --from-file=config.yaml="${KIND_DIR}/configs/ai-baseline.yaml" \
  -n default --dry-run=client -o yaml | kubectl --context "${CTX}" apply -f -
# praxis-ai:dev has ENTRYPOINT [praxis-ai] — needs -c arg
kubectl --context "${CTX}" patch deployment praxis-proxy -n default --type=json \
  -p '[{"op":"replace","path":"/spec/template/spec/containers/0/image","value":"praxis-ai:dev"},{"op":"replace","path":"/spec/template/spec/containers/0/args","value":["-c","/etc/praxis/config.yaml"]}]'
kubectl --context "${CTX}" set env deployment/praxis-proxy OTEL_EXPORTER_OTLP_ENDPOINT- -n default 2>/dev/null || true
kubectl --context "${CTX}" scale deployment/praxis-proxy --replicas=0 -n default
sleep 3
kubectl --context "${CTX}" scale deployment/praxis-proxy --replicas=1 -n default
kubectl --context "${CTX}" -n default wait --for=condition=Available deployment/praxis-proxy --timeout 60s
sleep 5

echo "Warmup..."
printf 'POST %s/v1/chat/completions\nContent-Type: application/json\n@%s\n' \
  "${GATEWAY_URL}" "${SCRIPT_DIR}/ai-payload.json" | \
  vegeta attack -rate=100 -duration=10s > /dev/null 2>&1 || true
sleep 2

for i in $(seq 1 "${RUNS}"); do
  run_vegeta "ai-baseline" "${i}"
  sleep 5
done

# ---- Run B: AI OTel noop (spans created, not exported) ----
echo "=========================================="
echo "  Run B: AI OTel noop (praxis-ai:dev-otel, no endpoint)"
echo "=========================================="

kubectl --context "${CTX}" create configmap praxis-config \
  --from-file=config.yaml="${KIND_DIR}/configs/ai-otel-noop.yaml" \
  -n default --dry-run=client -o yaml | kubectl --context "${CTX}" apply -f -
# praxis-ai:dev-otel has ENTRYPOINT [praxis -c /etc/praxis/config.yaml] — clear args
kubectl --context "${CTX}" patch deployment praxis-proxy -n default --type=json \
  -p '[{"op":"replace","path":"/spec/template/spec/containers/0/image","value":"praxis-ai:dev-otel"},{"op":"replace","path":"/spec/template/spec/containers/0/args","value":[]}]'
kubectl --context "${CTX}" set env deployment/praxis-proxy OTEL_EXPORTER_OTLP_ENDPOINT- -n default 2>/dev/null || true
kubectl --context "${CTX}" scale deployment/praxis-proxy --replicas=0 -n default
sleep 3
kubectl --context "${CTX}" scale deployment/praxis-proxy --replicas=1 -n default
kubectl --context "${CTX}" -n default wait --for=condition=Available deployment/praxis-proxy --timeout 60s
sleep 5

echo "Warmup..."
printf 'POST %s/v1/chat/completions\nContent-Type: application/json\n@%s\n' \
  "${GATEWAY_URL}" "${SCRIPT_DIR}/ai-payload.json" | \
  vegeta attack -rate=100 -duration=10s > /dev/null 2>&1 || true
sleep 2

for i in $(seq 1 "${RUNS}"); do
  run_vegeta "ai-otel-noop" "${i}"
  sleep 5
done

# ---- Run C: AI OTel full (spans exported to collector) ----
echo "=========================================="
echo "  Run C: AI OTel full (praxis-ai:dev-otel, exporting)"
echo "=========================================="

kubectl --context "${CTX}" create configmap praxis-config \
  --from-file=config.yaml="${KIND_DIR}/configs/ai-otel-full.yaml" \
  -n default --dry-run=client -o yaml | kubectl --context "${CTX}" apply -f -
kubectl --context "${CTX}" set env deployment/praxis-proxy \
  OTEL_EXPORTER_OTLP_ENDPOINT=http://otel-collector.otel.svc:4317 -n default
kubectl --context "${CTX}" scale deployment/praxis-proxy --replicas=0 -n default
sleep 3
kubectl --context "${CTX}" scale deployment/praxis-proxy --replicas=1 -n default
kubectl --context "${CTX}" -n default wait --for=condition=Available deployment/praxis-proxy --timeout 60s
sleep 5

echo "Warmup..."
printf 'POST %s/v1/chat/completions\nContent-Type: application/json\n@%s\n' \
  "${GATEWAY_URL}" "${SCRIPT_DIR}/ai-payload.json" | \
  vegeta attack -rate=100 -duration=10s > /dev/null 2>&1 || true
sleep 2

for i in $(seq 1 "${RUNS}"); do
  run_vegeta "ai-otel-full" "${i}"
  sleep 5
done

echo "=========================================="
echo "  AI Benchmark complete"
echo "=========================================="
echo "Results in: ${RESULTS_DIR}"
echo ""
echo "Generate report:"
echo "  bash ${SCRIPT_DIR}/report-ai.sh ${RESULTS_DIR}"
