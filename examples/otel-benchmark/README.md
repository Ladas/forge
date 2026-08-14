# OTel Observability Benchmark

Deploys a full observability stack on KIND for benchmarking Praxis proxy
OTel tracing overhead.

## Stack

- **Prometheus + Grafana** (kube-prometheus-stack) — metrics + visualization
- **Tempo** — distributed trace storage
- **Loki + Promtail** — log aggregation
- **OTel Collector** — trace pipeline (OTLP → Tempo + MLflow)
- **MLflow** — experiment tracking
- **Fortio echo** — mock HTTP backend
- **llm-d inference-sim** — mock LLM backend
- **Praxis proxy** — the proxy under test (baseline + OTel variants)

## Prerequisites

- Docker or Podman
- [KIND](https://kind.sigs.k8s.io/)
- [Helm](https://helm.sh/) with repos: `prometheus-community`, `grafana`, `community-charts`
- [vegeta](https://github.com/tsenart/vegeta) (for benchmarks)
- Praxis source checkout with OTel PRs (for image builds)

## Quick Start

```bash
# 1. Add helm repos (one-time)
helm repo add prometheus-community https://prometheus-community.github.io/helm-charts
helm repo add grafana https://grafana.github.io/helm-charts
helm repo add community-charts https://community-charts.github.io/helm-charts
helm repo update

# 2. Set the path to your praxis repo checkout
export PRAXIS_DIR=/path/to/praxis

# 3. Build praxis images (if not already built)
cd "$PRAXIS_DIR"
docker build -t praxis:dev -f Containerfile .
sed 's|cargo build --release -p praxis-proxy|cargo build --release -p praxis-proxy --features otel|g' \
  Containerfile | docker build -t praxis:dev-otel -f - .

# 4. Deploy the full stack
cd /path/to/forge
cargo run -- up --config examples/otel-benchmark.yaml

# If forge up doesn't apply stacks automatically, apply them manually:
for stack in prometheus tempo loki otel-collector mlflow mock-backends praxis-images praxis-deploy dashboards datasources; do
  cargo run -- stack apply --config examples/otel-benchmark.yaml local "$stack"
done

# 5. Switch to OTel image (if deployed with baseline)
kubectl --context kind-otel-bench-local set image deployment/praxis-proxy praxis-proxy=praxis:dev-otel

# 6. Verify
curl http://localhost:18080/         # Praxis proxy
open http://localhost:13000          # Grafana (admin/admin)
open http://localhost:19090          # Prometheus
open http://localhost:15000          # MLflow
```

## Run Benchmark

> **Note:** This benchmark uses the core `praxis` proxy, which generates
> a root request span, per-filter child spans, and an upstream exchange
> span (10 spans per request). To see AI-specific routing spans
> (`routing.select` with provider/cluster/site attributes), build from
> the `praxis-proxy/ai` repo with `--features opentelemetry` and use
> AI filters (intelligent_route, format classification) with
> inference-sim as the backend. That is a separate demo configuration.

```bash
bash examples/otel-benchmark/scripts/benchmark.sh
```

Runs 3 configurations at 2000 RPS for 30s each:
- **A: Baseline** — `praxis:dev` (no OTel feature)
- **B: OTel noop** — `praxis:dev-otel` (spans created, not exported)
- **C: OTel full** — `praxis:dev-otel` (spans exported to collector → Tempo)

Generate the markdown report:
```bash
bash examples/otel-benchmark/scripts/report.sh <results-dir>
```

## Dashboards

| Dashboard | URL | What it shows |
|-----------|-----|------|
| Praxis Proxy Overview | http://localhost:13000/d/praxis-proxy-overview | Request rate, latency P50/P99, requests by method |
| OTel Traces | http://localhost:13000/d/praxis-traces | Searchable trace table with clickable Trace IDs |
| Benchmark Results | http://localhost:13000/d/praxis-benchmark | CPU/memory for praxis + collector, RPS, latency |
| AI/LLM Golden Signals | http://localhost:13000/d/praxis-ai-golden-signals | P95 latency stat, throughput, AI token metrics (future) |
| Structured Logs | http://localhost:13000/d/praxis-logs | Log volume, error logs, all praxis access logs |

### Explore views

| View | URL |
|------|-----|
| Tempo trace search | http://localhost:13000/explore (select Tempo datasource → Search tab) |
| Prometheus metrics | http://localhost:13000/explore (select Prometheus datasource) |
| Loki log search | http://localhost:13000/explore (select Loki datasource) |

### Other UIs

| Service | URL |
|---------|-----|
| Prometheus | http://localhost:19090 |
| MLflow | http://localhost:15000 |
| Praxis proxy | http://localhost:18080 |
| Praxis admin/metrics | http://localhost:18901/metrics |

## Host Ports

| Port | Service | KIND NodePort |
|------|---------|------|
| 18080 | Praxis proxy | 30080 |
| 18901 | Praxis admin | 30901 |
| 13000 | Grafana | 30300 |
| 19090 | Prometheus | 30909 |
| 15000 | MLflow | 30500 |

## Known Issues

- **Grafana version**: Must use 11.x (pinned via `grafana.image.tag`). Grafana 12.0 has rendering bugs with provisioned dashboards using `row`/`gauge` panel types.
- **Datasources**: Prometheus and Tempo datasources are added via the `datasources` stack. If Grafana restarts, they need re-adding.
- **Tokio runtime fix**: The praxis `otel` feature requires a persistent Tokio runtime in `core/src/logging.rs` for the `BatchSpanProcessor` to drive tonic's async gRPC export. This fix is not yet in any upstream PR.

## Teardown

```bash
cargo run -- down --config examples/otel-benchmark.yaml
# or
kind delete cluster --name otel-bench-local
```
