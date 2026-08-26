//! Path-independence smoke tests.
//!
//! Verifies that:
//! - `--config /absolute/path config validate` works from a foreign directory;
//! - `.forge` state paths are deterministic and relative to `--state-dir`;
//! - no Grid checkout or specific working directory is required.

#![allow(clippy::tests_outside_test_module, reason = "integration tests live in tests/")]

use std::path::Path;

// ---------------------------------------------------------------
// Binary resolution
// ---------------------------------------------------------------

/// Resolve the praxis-forge binary built by this workspace.
fn forge_binary() -> std::path::PathBuf {
    let bin = Path::new(env!("CARGO_BIN_EXE_praxis-forge"));
    assert!(bin.exists(), "praxis-forge binary not found at {}", bin.display());
    bin.to_path_buf()
}

/// Absolute path to the fixtures directory.
fn fixtures_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

// ---------------------------------------------------------------
// Path-independence tests
// ---------------------------------------------------------------

#[test]
fn validate_with_absolute_path_from_foreign_directory() {
    let config_path = fixtures_dir().join("glb-demo.yaml");
    let output = std::process::Command::new(forge_binary())
        .arg("--config")
        .arg(&config_path)
        .arg("config")
        .arg("validate")
        .current_dir(std::env::temp_dir())
        .output()
        .unwrap_or_else(|_| std::process::abort());
    assert!(
        output.status.success(),
        "validate should succeed from foreign directory: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_all_fixtures_with_absolute_paths() {
    for fixture in [
        "glb-demo.yaml",
        "combined-site.yaml",
        "llmd-pool-metrics.yaml",
        "maas-ipp.yaml",
    ] {
        let config_path = fixtures_dir().join(fixture);
        let output = std::process::Command::new(forge_binary())
            .arg("--config")
            .arg(&config_path)
            .arg("config")
            .arg("validate")
            .current_dir(std::env::temp_dir())
            .output()
            .unwrap_or_else(|_| std::process::abort());
        assert!(
            output.status.success(),
            "{fixture}: validate should succeed with absolute path: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn config_show_works_from_foreign_directory() {
    let config_path = fixtures_dir().join("maas-ipp.yaml");
    let output = std::process::Command::new(forge_binary())
        .arg("--config")
        .arg(&config_path)
        .arg("config")
        .arg("show")
        .current_dir(std::env::temp_dir())
        .output()
        .unwrap_or_else(|_| std::process::abort());
    assert!(
        output.status.success(),
        "config show should succeed from foreign directory: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("maas-ipp"),
        "output should contain environment name: {stdout}"
    );
}

#[test]
fn config_schema_works_without_config_file() {
    let output = std::process::Command::new(forge_binary())
        .arg("config")
        .arg("schema")
        .current_dir(std::env::temp_dir())
        .output()
        .unwrap_or_else(|_| std::process::abort());
    assert!(
        output.status.success(),
        "config schema should work without any config file: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("ForgeConfig"),
        "schema output should contain ForgeConfig: {stdout}"
    );
}

#[test]
fn version_flag_prints_version() {
    let output = std::process::Command::new(forge_binary())
        .arg("--version")
        .current_dir(std::env::temp_dir())
        .output()
        .unwrap_or_else(|_| std::process::abort());
    assert!(
        output.status.success(),
        "--version should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("praxis-forge"),
        "--version should print praxis-forge: {stdout}"
    );
}

#[test]
fn state_dir_flag_is_deterministic() {
    let dir = tempfile::tempdir().unwrap_or_else(|_| std::process::abort());
    // `status` runs `kind get clusters`. A stub `kind` prepended to PATH
    // answers with an empty cluster list, so the probe is deterministic on
    // every machine instead of depending on a real kind installation (absent
    // on CI runners) with a reachable daemon.
    let stub_path = stub_kind_path(dir.path());
    let config_path = fixtures_dir().join("glb-demo.yaml");

    let empty_dir = dir.path().join("empty-state");
    let seeded_dir = dir.path().join("seeded-state");
    std::fs::create_dir_all(&seeded_dir).unwrap_or_else(|_| std::process::abort());
    std::fs::write(seeded_dir.join("state.json"), SEEDED_STATE).unwrap_or_else(|_| std::process::abort());

    // Path independence: the same state dir must give the same answer from any
    // working directory.
    let from_tmp = status_json(&config_path, &seeded_dir, &std::env::temp_dir(), &stub_path);
    let from_fixtures = status_json(&config_path, &seeded_dir, &fixtures_dir(), &stub_path);
    assert_eq!(
        from_tmp, from_fixtures,
        "status output must not depend on the working directory"
    );

    // The flag itself: reading a seeded state dir must differ from reading an
    // empty one. Without this the test passes with --state-dir deleted.
    let from_empty = status_json(&config_path, &empty_dir, &std::env::temp_dir(), &stub_path);
    assert_ne!(
        from_tmp, from_empty,
        "--state-dir must select which state is read, but both dirs gave the same output"
    );
    assert!(
        from_tmp.contains("glb-demo-net"),
        "status should reflect the seeded state dir, got: {from_tmp}"
    );
}

/// A minimal state file naming a network, used to prove `--state-dir` is read.
const SEEDED_STATE: &str = concat!(
    r#"{"apiVersion":"forge.praxis.dev/state/v1alpha1","#,
    r#""network":{"name":"glb-demo-net","phase":"active","cidr":"172.30.0.0/16"}}"#
);

/// Install a stub `kind` reporting an empty cluster list; return a PATH
/// value that resolves the stub before any real installation.
fn stub_kind_path(dir: &Path) -> std::ffi::OsString {
    let stub_dir = dir.join("stub-bin");
    std::fs::create_dir_all(&stub_dir).unwrap_or_else(|_| std::process::abort());
    let stub = stub_dir.join("kind");
    std::fs::write(&stub, "#!/bin/sh\nexit 0\n").unwrap_or_else(|_| std::process::abort());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|_| std::process::abort());
    }
    let existing = std::env::var_os("PATH").unwrap_or_default();
    let entries = std::iter::once(stub_dir).chain(std::env::split_paths(&existing));
    std::env::join_paths(entries).unwrap_or_else(|_| std::process::abort())
}

/// Run `status --output json` from `cwd` with `path_env` as PATH and
/// return stdout.
fn status_json(config_path: &Path, state_dir: &Path, cwd: &Path, path_env: &std::ffi::OsStr) -> String {
    let output = std::process::Command::new(forge_binary())
        .arg("--config")
        .arg(config_path)
        .arg("--state-dir")
        .arg(state_dir)
        .arg("--output")
        .arg("json")
        .arg("status")
        .env("PATH", path_env)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|_| std::process::abort());
    assert!(
        output.status.success(),
        "status with custom state-dir should succeed from {}: {}",
        cwd.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn doctor_runs_from_foreign_directory() {
    let config_path = fixtures_dir().join("glb-demo.yaml");
    let output = std::process::Command::new(forge_binary())
        .arg("--config")
        .arg(&config_path)
        .arg("doctor")
        .current_dir(std::env::temp_dir())
        .output()
        .unwrap_or_else(|_| std::process::abort());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Doctor must render its full report regardless of the working
    // directory (and regardless of which tools this runner has).
    for tool in ["docker", "podman", "kind", "kubectl", "helm"] {
        assert!(stdout.contains(tool), "doctor should report {tool}: {stdout}");
    }
    // Exit-code contract: doctor fails exactly when a required tool
    // is missing or no container runtime (docker/podman) was found.
    assert_eq!(
        output.status.success(),
        doctor_report_is_healthy(&stdout),
        "doctor exit code must match its report: {stdout}"
    );
}

/// Derive doctor's health verdict from its text report.
fn doctor_report_is_healthy(stdout: &str) -> bool {
    let missing_required = stdout
        .lines()
        .any(|line| line.contains("MISSING") && line.contains("(required)"));
    let runtime_found = stdout.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("ok: docker") || trimmed.starts_with("ok: podman")
    });
    !missing_required && runtime_found
}

#[test]
fn config_init_dry_run_from_foreign_directory() {
    let output = std::process::Command::new(forge_binary())
        .arg("config")
        .arg("init")
        .arg("--dry-run")
        .current_dir(std::env::temp_dir())
        .output()
        .unwrap_or_else(|_| std::process::abort());
    assert!(
        output.status.success(),
        "config init --dry-run should succeed from foreign directory: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn invalid_config_returns_nonzero_exit_code() {
    let dir = tempfile::tempdir().unwrap_or_else(|_| std::process::abort());
    let bad_config = dir.path().join("bad.yaml");
    std::fs::write(&bad_config, "apiVersion: wrong/v1\nkind: Wrong\n").unwrap_or_else(|_| std::process::abort());
    let output = std::process::Command::new(forge_binary())
        .arg("--config")
        .arg(&bad_config)
        .arg("config")
        .arg("validate")
        .current_dir(std::env::temp_dir())
        .output()
        .unwrap_or_else(|_| std::process::abort());
    assert!(
        !output.status.success(),
        "invalid config should return nonzero exit code"
    );
}
