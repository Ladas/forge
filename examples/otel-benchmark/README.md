# OTel Observability Benchmark

Deploys a full observability stack on KIND for benchmarking Praxis proxy
OTel tracing overhead across two scenarios: core proxy and AI proxy.

## Stack

- **Prometheus + Grafana 11.x** — metrics + visualization
- **Tempo** — distributed trace storage
- **Loki + Promtail** — log aggregation
- **OTel Collector** — trace pipeline (OTLP -> Tempo)
- **MLflow** — experiment tracking UI
- **Fortio echo** — mock HTTP backend
- **llm-d inference-sim** — mock LLM backend
- **Praxis proxy** — the proxy under test (baseline + OTel variants)

## Prerequisites

- Docker or Podman
- [KIND](https://kind.sigs.k8s.io/)
- [Helm](https://helm.sh/) with repos: `prometheus-community`, `grafana`, `community-charts`
- [vegeta](https://github.com/tsenart/vegeta) (for benchmarks)
- Praxis source checkouts (for image builds)

## Quick Start

```bash
# 1. Add helm repos (one-time)
helm repo add prometheus-community https://prometheus-community.github.io/helm-charts
helm repo add grafana https://grafana.github.io/helm-charts
helm repo add community-charts https://community-charts.github.io/helm-charts
helm repo update

# 2. Set paths to source checkouts
export PRAXIS_DIR=/path/to/praxis    # praxis core repo (with OTel PRs)
export AI_DIR=/path/to/ai            # praxis AI repo

# 3. Build images
# Core: baseline + OTel
cd "$PRAXIS_DIR"
docker build -t praxis:dev -f Containerfile .
sed 's|cargo build --release -p praxis-proxy|cargo build --release -p praxis-proxy --features otel|g' \
  Containerfile | docker build -t praxis:dev-otel -f - .

# AI: baseline (from upstream/main, no patches)
cd "$AI_DIR"
docker build -t praxis-ai:dev -f Containerfile .

# AI: OTel (from otel-fixes branch with praxis core patches)
# See "Building praxis-ai:dev-otel" section below.

# 4. Deploy the full stack
cd /path/to/forge
praxis-forge doctor                                            # check tools
praxis-forge plan --config examples/otel-benchmark.yaml        # preview
praxis-forge up --config examples/otel-benchmark.yaml          # create cluster

# Load pre-built images into KIND
kind load docker-image praxis:dev praxis:dev-otel --name otel-bench-local
kind load docker-image praxis-ai:dev praxis-ai:dev-otel --name otel-bench-local

# Apply stacks (skip praxis-images if images are already loaded)
for stack in prometheus tempo loki otel-collector mlflow mock-backends praxis-deploy dashboards datasources; do
  praxis-forge apply --config examples/otel-benchmark.yaml local "$stack"
done
praxis-forge status --config examples/otel-benchmark.yaml      # check status

# 5. Verify
curl http://localhost:18080/
open http://localhost:13000    # Grafana (admin/admin)
open http://localhost:19090    # Prometheus
open http://localhost:15000    # MLflow
```

## Scenario 1: Core Praxis OTel Benchmark

Measures OTel tracing overhead on the core proxy with echo backend.
Generates 10 spans per request (root + 8 per-filter + upstream_exchange).

```bash
bash examples/otel-benchmark/scripts/benchmark.sh
```

Runs 3 configurations at 2000 RPS for 30s each:
- **A: Baseline** — `praxis:dev` (no OTel feature)
- **B: OTel noop** — `praxis:dev-otel` (spans created, not exported)
- **C: OTel full** — `praxis:dev-otel` (spans exported to collector -> Tempo)

Generate the report:
```bash
bash examples/otel-benchmark/scripts/report.sh <results-dir>
```

## Scenario 2: AI Praxis with Inference Sim

Measures OTel overhead on the AI proxy with mock LLM backend.
Generates 11 spans per request (core spans + response_body phase).
Sends `POST /v1/chat/completions` to inference-sim.

```bash
bash examples/otel-benchmark/scripts/benchmark-ai.sh
```

Runs 3 configurations at 500 RPS for 30s each:
- **A: AI Baseline** — `praxis-ai:dev` (no OTel feature)
- **B: AI OTel noop** — `praxis-ai:dev-otel` (spans created, not exported)
- **C: AI OTel full** — `praxis-ai:dev-otel` (spans exported to collector -> Tempo)

Generate the report:
```bash
bash examples/otel-benchmark/scripts/report-ai.sh <results-dir>
```

### Span tree (AI request)

```
POST /v1/chat/completions -> inference-sim  (root)
  |-- filter:request_id:request
  |-- filter:access_log:request
  |-- filter:router:request          -> routes /v1/* to inference cluster
  |-- filter:load_balancer:request
  |-- filter:load_balancer:response
  |-- filter:router:response
  |-- filter:access_log:response
  |-- filter:request_id:response
  |-- filter:access_log:response_body
  +-- upstream_exchange [inference-sim:8000]
```

## Building praxis-ai:dev-otel

The AI OTel image requires patched praxis core (for OTel spans) and the
AI OTel fixes. Build with both repos side by side:

```bash
BUILD_DIR=$(mktemp -d)
rsync -a --exclude='.git' --exclude='target' "$AI_DIR/" "$BUILD_DIR/ai/"
for crate in core filter protocol tls server; do
    rsync -a --exclude='target' "$PRAXIS_DIR/$crate/" "$BUILD_DIR/praxis/$crate/"
done
cp "$PRAXIS_DIR/Cargo.toml" "$PRAXIS_DIR/Cargo.lock" "$BUILD_DIR/praxis/"

# Add patch.crates-io to ai/Cargo.toml pointing to ../praxis/*
cat >> "$BUILD_DIR/ai/Cargo.toml" << 'PATCH'

[patch.crates-io]
praxis-proxy-core = { path = "../praxis/core" }
praxis-proxy-filter = { path = "../praxis/filter" }
praxis-proxy-protocol = { path = "../praxis/protocol" }
praxis-proxy-tls = { path = "../praxis/tls" }
praxis-proxy = { path = "../praxis/server" }
PATCH

# Build with OTel features enabled
# (requires custom Containerfile that copies both repos)
docker build -t praxis-ai:dev-otel -f Containerfile "$BUILD_DIR"
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
| Tempo trace search | http://localhost:13000/explore (select Tempo datasource) |
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
- **Tokio runtime fix**: The praxis `otel` feature requires a persistent Tokio runtime in `core/src/logging.rs` for the `BatchSpanProcessor` to drive tonic's async gRPC export.
- **MLflow trace ingestion**: The OTel collector v0.108 sends protobuf to MLflow, but MLflow 3.x only accepts JSON OTLP. Traces go to Tempo (primary store). MLflow shows the experiment tracking UI.

## Teardown

```bash
praxis-forge down --config examples/otel-benchmark.yaml
# or
kind delete cluster --name otel-bench-local
```
