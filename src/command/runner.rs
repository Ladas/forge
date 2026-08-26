//! Mockable command execution abstraction.
//!
//! External tool invocations go through [`CommandRunner`] so tests
//! can inject a `MockRunner` and verify calls without side effects.

use std::{collections::BTreeMap, ffi::OsString, fmt};

use crate::error::ForgeError;

/// Abstraction over external command execution.
pub trait CommandRunner {
    /// Execute an external command and return its output.
    ///
    /// # Errors
    ///
    /// Returns [`ForgeError`] if the command fails to execute.
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, ForgeError>;
}

/// Specification for a single external command invocation.
#[derive(Clone, Debug)]
pub struct CommandSpec {
    /// Program to execute.
    pub program: OsString,
    /// Command-line arguments.
    pub args: Vec<OsString>,
    /// Environment variables to set.
    pub env: BTreeMap<OsString, OsString>,
    /// Optional standard input bytes.
    pub stdin: Option<Vec<u8>>,
    /// Values that must not appear in display output.  Each value is
    /// replaced wherever it occurs within an argument, so secrets
    /// embedded in larger arguments (`--set token=SECRET`) are caught.
    pub redact: Vec<Redaction>,
}

/// A value to redact from display output.
#[derive(Clone, Debug)]
pub struct Redaction {
    /// The literal value to replace with `[REDACTED]`.
    pub value: OsString,
}

/// Output from a completed command.
#[derive(Clone, Debug)]
pub struct CommandOutput {
    /// Process exit code (0 = success).
    pub status: i32,
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error.
    pub stderr: String,
}

impl fmt::Display for CommandSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.program.to_string_lossy())?;
        for arg in &self.args {
            write!(f, " {}", redact_value(arg, &self.redact))?;
        }
        Ok(())
    }
}

/// Replace every occurrence of a redacted value with `[REDACTED]`.
///
/// Matching is by substring on the lossy display form, so a secret
/// embedded in a larger argument is redacted, not just an argument
/// that equals the secret exactly.
fn redact_value(value: &OsString, redactions: &[Redaction]) -> String {
    let mut lossy = value.to_string_lossy().into_owned();
    for rd in redactions {
        let needle = rd.value.to_string_lossy();
        if !needle.is_empty() {
            lossy = lossy.replace(needle.as_ref(), "[REDACTED]");
        }
    }
    lossy
}

// -----------------------------------------------------------------
// System runner (real process execution)
// -----------------------------------------------------------------

/// Real command runner that executes via [`std::process::Command`].
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, ForgeError> {
        let mut cmd = build_process(spec);
        let output = run_process(&mut cmd, spec)?;
        Ok(into_command_output(&output))
    }
}

/// Build a [`std::process::Command`] from a [`CommandSpec`].
fn build_process(spec: &CommandSpec) -> std::process::Command {
    let mut cmd = std::process::Command::new(&spec.program);
    cmd.args(&spec.args);
    for (key, val) in &spec.env {
        cmd.env(key, val);
    }
    configure_stdio(&mut cmd, spec.stdin.is_some());
    cmd
}

/// Set up process stdio handles.
fn configure_stdio(cmd: &mut std::process::Command, pipe_stdin: bool) {
    if pipe_stdin {
        cmd.stdin(std::process::Stdio::piped());
    }
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
}

/// Execute a prepared command, optionally piping stdin data.
fn run_process(cmd: &mut std::process::Command, spec: &CommandSpec) -> Result<std::process::Output, ForgeError> {
    match &spec.stdin {
        Some(data) => run_with_stdin(cmd, data, spec),
        None => cmd.output().map_err(|err| command_error(spec, &err)),
    }
}

/// Spawn a child process and write data to its standard input.
///
/// Stdin is written from a separate thread while the parent drains
/// stdout/stderr via `wait_with_output`.  Writing synchronously first
/// would deadlock once the child fills the OS pipe buffer with output
/// before consuming all of its stdin (e.g. `kubectl apply -f -` on a
/// large multi-document manifest).
fn run_with_stdin(
    cmd: &mut std::process::Command,
    data: &[u8],
    spec: &CommandSpec,
) -> Result<std::process::Output, ForgeError> {
    let mut child = cmd.spawn().map_err(|err| command_error(spec, &err))?;
    let writer = spawn_stdin_writer(&mut child, data);
    let output = child.wait_with_output().map_err(|err| command_error(spec, &err))?;

    match join_stdin_writer(writer) {
        Ok(()) => Ok(output),
        // A child can reject the command and close stdin before the parent
        // finishes writing. Preserve its exit status and captured stderr so
        // the caller reports the primary failure instead of a secondary EPIPE.
        Err(_error) if !output.status.success() => Ok(output),
        Err(error) => Err(command_error(spec, &error)),
    }
}

/// Handle to a background thread writing a child's stdin.
type StdinWriter = std::thread::JoinHandle<std::io::Result<()>>;

/// Spawn a thread that writes data to the child's stdin and closes it.
fn spawn_stdin_writer(child: &mut std::process::Child, data: &[u8]) -> Option<StdinWriter> {
    let mut stdin = child.stdin.take()?;
    let owned = data.to_vec();
    Some(std::thread::spawn(move || {
        use std::io::Write as _;
        stdin.write_all(&owned)
    }))
}

/// Join the stdin writer thread, mapping a panic to an IO error.
fn join_stdin_writer(writer: Option<StdinWriter>) -> std::io::Result<()> {
    match writer {
        None => Ok(()),
        Some(handle) => handle
            .join()
            .unwrap_or_else(|_| Err(std::io::Error::other("stdin writer thread panicked"))),
    }
}

/// Convert a process output reference into a [`CommandOutput`].
fn into_command_output(output: &std::process::Output) -> CommandOutput {
    CommandOutput {
        status: status_code(output.status),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// Map an exit status to a numeric code.
///
/// A child killed by a signal has no exit code; following shell
/// convention it is reported as 128 + the signal number so error
/// messages name the real termination cause instead of a fake `-1`.
fn status_code(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or_else(|| signal_status_code(status))
}

/// Fold a fatal signal into a shell-convention exit code (Unix).
#[cfg(unix)]
fn signal_status_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt as _;
    status.signal().map_or(-1, |sig| 128_i32.saturating_add(sig))
}

/// Non-Unix platforms have no signals to report.
#[cfg(not(unix))]
fn signal_status_code(_status: std::process::ExitStatus) -> i32 {
    -1
}

/// Build a [`ForgeError::Command`] from a spec and IO error.
fn command_error(spec: &CommandSpec, err: &std::io::Error) -> ForgeError {
    ForgeError::Command {
        program: spec.program.to_string_lossy().into_owned(),
        message: err.to_string(),
    }
}

// -----------------------------------------------------------------
// Mock runner for tests
// -----------------------------------------------------------------

/// A test-only command runner that records calls and returns canned responses.
#[cfg(any(test, feature = "test-support"))]
pub struct MockRunner {
    /// Canned responses keyed by display string or program name.
    responses: BTreeMap<String, CommandOutput>,
    /// Recorded calls for assertion.
    calls: std::cell::RefCell<Vec<CommandSpec>>,
}

#[cfg(any(test, feature = "test-support"))]
impl Default for MockRunner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl MockRunner {
    /// Create a new mock runner with no responses.
    pub fn new() -> Self {
        Self {
            responses: BTreeMap::new(),
            calls: std::cell::RefCell::new(Vec::new()),
        }
    }

    /// Register a canned response for a display string or program.
    pub fn respond(&mut self, program: &str, output: CommandOutput) -> &mut Self {
        self.responses.insert(program.to_owned(), output);
        self
    }

    /// Return all recorded calls.
    pub fn calls(&self) -> Vec<CommandSpec> {
        self.calls.borrow().clone()
    }

    /// Return the number of recorded calls.
    pub fn call_count(&self) -> usize {
        self.calls.borrow().len()
    }

    /// Return recorded calls whose display string contains the pattern.
    pub fn calls_matching(&self, pattern: &str) -> Vec<CommandSpec> {
        self.calls
            .borrow()
            .iter()
            .filter(|call| format!("{call}").contains(pattern))
            .cloned()
            .collect()
    }

    /// Check if any recorded call's display string contains the pattern.
    pub fn was_called(&self, pattern: &str) -> bool {
        self.calls
            .borrow()
            .iter()
            .any(|call| format!("{call}").contains(pattern))
    }

    /// Clear all recorded calls.
    pub fn clear_calls(&self) {
        self.calls.borrow_mut().clear();
    }
}

#[cfg(any(test, feature = "test-support"))]
impl CommandRunner for MockRunner {
    /// Record the call, then look up by display string first, then
    /// by program name alone.
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, ForgeError> {
        self.calls.borrow_mut().push(spec.clone());
        let display = format!("{spec}");
        if let Some(output) = self.responses.get(&display) {
            return Ok(output.clone());
        }
        let program = spec.program.to_string_lossy();
        self.responses
            .get(program.as_ref())
            .cloned()
            .ok_or_else(|| ForgeError::Command {
                program: program.into_owned(),
                message: "not found".to_owned(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_redacts_sensitive_args() {
        let spec = CommandSpec {
            program: "helm".into(),
            args: vec!["install".into(), "s3cr3t-token".into()],
            env: BTreeMap::new(),
            stdin: None,
            redact: vec![Redaction {
                value: "s3cr3t-token".into(),
            }],
        };
        let display = format!("{spec}");
        assert!(display.contains("[REDACTED]"), "should redact arg, got: {display}");
        assert!(!display.contains("s3cr3t"), "secret should not appear, got: {display}");
    }

    #[test]
    fn display_redacts_secret_embedded_in_larger_arg() {
        let spec = CommandSpec {
            program: "helm".into(),
            args: vec!["--set".into(), "token=s3cr3t-token".into()],
            env: BTreeMap::new(),
            stdin: None,
            redact: vec![Redaction {
                value: "s3cr3t-token".into(),
            }],
        };
        let display = format!("{spec}");
        assert_eq!(display, "helm --set token=[REDACTED]");
    }

    #[test]
    fn display_ignores_empty_redaction_value() {
        let spec = CommandSpec {
            program: "kubectl".into(),
            args: vec!["get".into()],
            env: BTreeMap::new(),
            stdin: None,
            redact: vec![Redaction { value: "".into() }],
        };
        let display = format!("{spec}");
        assert_eq!(display, "kubectl get", "an empty redaction must not mangle output");
    }

    #[test]
    fn display_preserves_non_redacted_args() {
        let spec = CommandSpec {
            program: "kubectl".into(),
            args: vec!["get".into(), "pods".into()],
            env: BTreeMap::new(),
            stdin: None,
            redact: Vec::new(),
        };
        let display = format!("{spec}");
        assert_eq!(display, "kubectl get pods");
    }

    #[test]
    fn mock_runner_returns_canned_response() {
        let mut runner = MockRunner::new();
        runner.respond(
            "kubectl",
            CommandOutput {
                status: 0,
                stdout: "/usr/bin/kubectl".to_owned(),
                stderr: String::new(),
            },
        );
        let spec = CommandSpec {
            program: "kubectl".into(),
            args: Vec::new(),
            env: BTreeMap::new(),
            stdin: None,
            redact: Vec::new(),
        };
        let result = runner.run(&spec).unwrap_or_else(|_| {
            std::process::abort();
            #[expect(unreachable_code, reason = "abort prevents reaching this")]
            {
                unreachable!()
            }
        });
        assert_eq!(result.status, 0, "status should be 0");
    }

    #[test]
    fn mock_runner_returns_error_for_unknown_program() {
        let runner = MockRunner::new();
        let spec = CommandSpec {
            program: "nonexistent".into(),
            args: Vec::new(),
            env: BTreeMap::new(),
            stdin: None,
            redact: Vec::new(),
        };
        let Err(err) = runner.run(&spec) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("not found"), "expected not-found error, got: {msg}");
    }

    #[test]
    fn mock_runner_records_calls() {
        let mut runner = MockRunner::new();
        runner.respond(
            "kubectl",
            CommandOutput {
                status: 0,
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        let spec = CommandSpec {
            program: "kubectl".into(),
            args: vec!["get".into(), "pods".into()],
            env: BTreeMap::new(),
            stdin: None,
            redact: Vec::new(),
        };
        let _result = runner.run(&spec);
        assert_eq!(runner.call_count(), 1, "should record one call");
    }

    #[test]
    fn mock_runner_was_called_matches() {
        let mut runner = MockRunner::new();
        runner.respond(
            "kind get clusters",
            CommandOutput {
                status: 0,
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        let spec = CommandSpec {
            program: "kind".into(),
            args: vec!["get".into(), "clusters".into()],
            env: BTreeMap::new(),
            stdin: None,
            redact: Vec::new(),
        };
        let _result = runner.run(&spec);
        assert!(runner.was_called("kind get clusters"), "should match display string");
        assert!(!runner.was_called("kind create"), "should not match unrelated command");
    }

    #[test]
    fn mock_runner_clear_calls_resets() {
        let mut runner = MockRunner::new();
        runner.respond(
            "kubectl",
            CommandOutput {
                status: 0,
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        let spec = CommandSpec {
            program: "kubectl".into(),
            args: Vec::new(),
            env: BTreeMap::new(),
            stdin: None,
            redact: Vec::new(),
        };
        let _result = runner.run(&spec);
        assert_eq!(runner.call_count(), 1, "should have one call before clear");
        runner.clear_calls();
        assert_eq!(runner.call_count(), 0, "should have zero calls after clear");
    }

    #[test]
    fn system_runner_preserves_child_error_when_stdin_closes_early() {
        let spec = CommandSpec {
            program: "sh".into(),
            args: vec!["-c".into(), "printf 'primary failure' >&2; exit 23".into()],
            env: BTreeMap::new(),
            stdin: Some(vec![b'x'; 1024 * 1024]),
            redact: Vec::new(),
        };

        let output = SystemRunner.run(&spec).unwrap_or_else(|_| std::process::abort());

        assert_eq!(output.status, 23);
        assert_eq!(output.stderr, "primary failure");
    }

    #[cfg(unix)]
    #[test]
    fn system_runner_reports_signal_termination_as_128_plus_signal() {
        let spec = CommandSpec {
            program: "sh".into(),
            args: vec!["-c".into(), "kill -TERM $$".into()],
            env: BTreeMap::new(),
            stdin: None,
            redact: Vec::new(),
        };

        let output = SystemRunner.run(&spec).unwrap_or_else(|_| std::process::abort());

        assert_eq!(output.status, 143, "SIGTERM (15) should report as 128 + 15");
    }

    #[test]
    fn system_runner_survives_child_flooding_output_before_reading_stdin() {
        // The child emits far more than the OS pipe buffer (~64 KiB)
        // before it reads any stdin. A parent that writes all stdin
        // before draining output deadlocks here.
        let flood: usize = 256 * 1024;
        let spec = CommandSpec {
            program: "sh".into(),
            args: vec!["-c".into(), format!("head -c {flood} /dev/zero; cat").into()],
            env: BTreeMap::new(),
            stdin: Some(vec![b'x'; flood]),
            redact: Vec::new(),
        };

        let output = SystemRunner.run(&spec).unwrap_or_else(|_| std::process::abort());

        assert_eq!(output.status, 0);
        assert_eq!(
            output.stdout.len(),
            flood.saturating_mul(2),
            "child should echo all stdin after the flood"
        );
    }

    #[test]
    fn system_runner_writes_stdin_to_successful_child() {
        let spec = CommandSpec {
            program: "sh".into(),
            args: vec!["-c".into(), "cat".into()],
            env: BTreeMap::new(),
            stdin: Some(b"manifest".to_vec()),
            redact: Vec::new(),
        };

        let output = SystemRunner.run(&spec).unwrap_or_else(|_| std::process::abort());

        assert_eq!(output.status, 0);
        assert_eq!(output.stdout, "manifest");
        assert!(output.stderr.is_empty());
    }
}
