//! Path-independence smoke tests.
//!
//! Verifies that:
//! - `--config /absolute/path config validate` works from a foreign directory;
//! - `.forge` state paths are deterministic and relative to `--state-dir`;
//! - no Grid checkout or specific working directory is required.

#![allow(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/"
)]

use std::path::Path;

// ---------------------------------------------------------------
// Binary resolution
// ---------------------------------------------------------------

/// Resolve the praxis-forge binary built by this workspace.
fn forge_binary() -> std::path::PathBuf {
    let bin = Path::new(env!("CARGO_BIN_EXE_praxis-forge"));
    assert!(
        bin.exists(),
        "praxis-forge binary not found at {}",
        bin.display()
    );
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
    // `status` shells out to `kind get clusters`. Without it the command fails
    // for reasons unrelated to path independence, so state the dependency
    // instead of asserting through it.
    if !binary_on_path("kind") {
        note_skip("state_dir_flag_is_deterministic: `kind` not on PATH");
        return;
    }
    let dir = tempfile::tempdir().unwrap_or_else(|_| std::process::abort());
    let state_dir = dir.path().join("custom-state");
    let config_path = fixtures_dir().join("glb-demo.yaml");

    // Determinism means the same answer regardless of where it is run from, so
    // the assertion has to compare two working directories, not just exit 0.
    let from_tmp = status_json(&config_path, &state_dir, &std::env::temp_dir());
    let from_fixtures = status_json(&config_path, &state_dir, &fixtures_dir());

    assert_eq!(
        from_tmp, from_fixtures,
        "status output must not depend on the working directory"
    );
    assert!(
        !from_tmp.trim().is_empty(),
        "status should emit a JSON document"
    );
}

/// Run `status --output json` from `cwd` and return stdout.
fn status_json(config_path: &Path, state_dir: &Path, cwd: &Path) -> String {
    let output = std::process::Command::new(forge_binary())
        .arg("--config")
        .arg(config_path)
        .arg("--state-dir")
        .arg(state_dir)
        .arg("--output")
        .arg("json")
        .arg("status")
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

/// Report a skipped test on stderr.
///
/// Written through [`std::io::Write`] rather than `eprintln!` because the crate
/// denies `clippy::print_stderr`, and a skip that says nothing is worse than the
/// lint it would dodge.
fn note_skip(message: &str) {
    use std::io::Write as _;
    let mut err = std::io::stderr();
    let _written = err.write_all(format!("skipping {message}\n").as_bytes());
}

/// Check whether an executable is resolvable on `PATH`.
fn binary_on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(name).is_file())
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
    assert!(
        output.status.success(),
        "doctor should succeed from foreign directory: {}",
        String::from_utf8_lossy(&output.stderr)
    );
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
    std::fs::write(&bad_config, "apiVersion: wrong/v1\nkind: Wrong\n")
        .unwrap_or_else(|_| std::process::abort());
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
