# Agent Guidance

This file provides guidance to coding agents when
working with code in this repository.

AI tools may assist with implementation, but do not
add Claude or another AI tool as a commit
collaborator, co-author, or signatory. Commit
sign-off belongs to the human contributor responsible
for the change.

## What This Is

Forge is a standalone CLI for composing multi-cluster
Kubernetes development environments from a single
YAML configuration. Binary name: `praxis-forge`.

Forge manages:
- KIND cluster lifecycle (`up`/`down`/`cluster`)
- Host-level container services
  (`service start`/`stop`/`logs`)
- Composable deployment stacks
  (`stack apply`/`plan`/`status`)
- Cross-cluster Docker networking
- Template-based manifest rendering with capture
  variables
- Persistent state under `.forge/`

Forge does not perform project-specific assertions,
CRD validation, or operator testing.

## Requirements

- Rust stable 1.96+
- Rust nightly (for `rustfmt`)
- `cargo-machete` (unused dependency detection)
- `cargo-audit`, `cargo-deny` (supply chain safety)
- `cargo-llvm-cov` (coverage, optional)

## Quick Reference

```console
make build          # workspace build
make check          # type-check only (fast)
make test           # all tests
make test V=1       # tests with --nocapture
make fmt            # format with nightly rustfmt
make lint           # clippy + fmt check + machete
make lint-extra     # typos + taplo + shellcheck +
                    # actionlint
make doc            # docs (warnings denied, private)
make audit          # cargo audit + cargo deny check
make coverage       # HTML coverage report
make coverage-check # fail if lines < 90% or
                    # regions < 80%
make all            # build + lint + lint-extra +
                    # test + audit
```

Single test:

```console
cargo test test_name
```

Run the CLI:

```console
cargo run --bin praxis-forge
```

## Architecture

Single-crate package. All modules live under `src/`.

```text
main.rs        CLI entry point (clap)
cli.rs         Clap arg parser and subcommand enum
context.rs     ForgeContext (shared runtime state)
config.rs      YAML config loading and ForgeConfig
config/        Schema, validation, deserialization
error.rs       ForgeError (thiserror)
output.rs      Text and JSON output formatting

command/       Subcommand implementations
  up.rs        Network + cluster + service bring-up
  down.rs      Teardown in reverse order
  status.rs    Environment status reporting
  doctor.rs    Prerequisite health checks
  config.rs    Config validation and schema export
  plan.rs      Dry-run planning
  runner.rs    Command execution and redaction

cluster/       KIND cluster management
  kind.rs      KIND CLI wrapper (create/delete)
  kubeconfig.rs  Kubeconfig merging and rewriting

service/       Container service management
  health.rs    Health check polling with duration
               parsing

networking.rs  Docker network lifecycle and
               ownership labels

stack/         Deployment stack engine
  engine.rs    Step executor (URL, Helm, Kustomize,
               Exec, ForEach, templates, etc.)
  steps.rs     Individual step implementations
  template.rs  Go-style template rendering

runtime.rs     Runtime config types
state/         Persistent state under .forge/
  lock.rs      File-based advisory locking
```

## Conventions

Coding conventions are defined in the [Praxis
conventions repository](https://github.com/praxis-proxy/conventions).
Forge-specific notes:

- Single crate (no sub-crates)
- `test-support` feature flag gates test-only code
- All `make` targets use `--features test-support`
- `mod_module_files` lint enforced: use `foo.rs` not
  `foo/mod.rs`
- `min_ident_chars` lint enforced: no single-char
  identifiers (use `err`, `val`, `ch`, etc.)
- `single_char_lifetime_names` lint enforced: use
  descriptive lifetime names (`'ctx`, `'env`, `'svc`)
- `too_many_lines` threshold: 30 lines per function
- `cognitive_complexity` threshold: 12
- `clippy.toml` sets `msrv = "1.96"`
- Coverage thresholds: 90% lines, 80% regions

## Test Requirements

Forge has unit tests alongside source modules and
integration tests under `tests/`.

- Unit tests go in `#[cfg(test)] mod tests` at the
  bottom of each source file
- Integration tests under `tests/` use the
  `test-support` feature
- Tests use mock `CommandRunner` implementations to
  avoid real KIND/Docker calls
- The `FakeRunner` records all command invocations
  for assertion

## Key Patterns

- **CommandRunner trait**: all shell commands go
  through `dyn CommandRunner` for testability
- **ForgeContext**: shared context passed through
  all operations (runner, config, state_dir, format,
  dry_run)
- **State machine**: clusters track phase transitions
  (Creating, Running, Deleting, Gone); services track
  Running, Unhealthy, Stopped, Gone
- **Reverse-order teardown**: `down` deletes services
  then clusters in reverse dependency order
- **Deterministic naming**: container names derived
  from environment name + service name
- **Stack digest**: SHA-256 of serialized stack spec
  detects config drift
