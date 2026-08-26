//! The `doctor` command: read-only tool availability check.
//!
//! Probes `PATH` for required and optional external tools without
//! creating, modifying, or deleting any resources.

use std::{collections::BTreeMap, io::Write};

use crate::{
    command::runner::{CommandOutput, CommandRunner, CommandSpec},
    error::ForgeError,
    output::{self, OutputFormat},
};

/// Tools that `doctor` probes for.
const TOOLS: &[ToolProbe] = &[
    ToolProbe {
        name: "docker",
        required: false,
    },
    ToolProbe {
        name: "podman",
        required: false,
    },
    ToolProbe {
        name: "kind",
        required: true,
    },
    ToolProbe {
        name: "kubectl",
        required: true,
    },
    ToolProbe {
        name: "helm",
        required: false,
    },
];

/// Metadata about one tool to probe.
struct ToolProbe {
    /// Tool name (also the binary name).
    name: &'static str,
    /// Whether the tool is required for basic operation.
    required: bool,
}

/// Result of probing a single tool.
#[derive(serde::Serialize)]
struct ToolStatus {
    /// Tool name.
    name: String,
    /// Whether the tool was found in `PATH`.
    found: bool,
    /// Whether the tool is required.
    required: bool,
    /// Path to the binary, if found.
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}

/// Run the `doctor` command.
///
/// Renders the probe results, then fails when any required tool is
/// missing or no container runtime (docker or podman) was found, so
/// scripts and CI gates can rely on the exit code.
///
/// # Errors
///
/// Returns [`ForgeError::Doctor`] naming the missing tools, or
/// another [`ForgeError`] if rendering fails.
pub fn run(runner: &dyn CommandRunner, format: &OutputFormat, writer: &mut dyn Write) -> Result<(), ForgeError> {
    let results = probe_tools(runner);
    render_results(&results, format, writer)?;
    ensure_healthy(&results)
}

/// Return an error naming the missing required tools, if any.
fn ensure_healthy(results: &[ToolStatus]) -> Result<(), ForgeError> {
    let missing = missing_tools(results);
    if missing.is_empty() {
        return Ok(());
    }
    Err(ForgeError::Doctor(format!(
        "missing required tools: {}",
        missing.join(", ")
    )))
}

/// Names of missing required tools, including the runtime group.
///
/// `docker` and `podman` are individually optional, but at least one
/// container runtime must be present for Forge to operate.
fn missing_tools(results: &[ToolStatus]) -> Vec<String> {
    let mut missing: Vec<String> = results
        .iter()
        .filter(|tool| tool.required && !tool.found)
        .map(|tool| tool.name.clone())
        .collect();
    let has_runtime = results
        .iter()
        .any(|tool| tool.found && (tool.name == "docker" || tool.name == "podman"));
    if !has_runtime {
        missing.push("docker or podman".to_owned());
    }
    missing
}

/// Probe all tools and collect results.
fn probe_tools(runner: &dyn CommandRunner) -> Vec<ToolStatus> {
    TOOLS.iter().map(|tool| probe_one(runner, tool)).collect()
}

/// Probe one tool by running `which <name>`.
fn probe_one(runner: &dyn CommandRunner, tool: &ToolProbe) -> ToolStatus {
    let spec = which_spec(tool.name);
    match runner.run(&spec) {
        Ok(out) if out.status == 0 => found_status(tool, &out),
        _ => missing_status(tool),
    }
}

/// Build a `which <name>` command spec.
fn which_spec(name: &str) -> CommandSpec {
    CommandSpec {
        program: "which".into(),
        args: vec![name.into()],
        env: BTreeMap::default(),
        stdin: None,
        redact: Vec::new(),
    }
}

/// Build a found-tool result.
fn found_status(tool: &ToolProbe, out: &CommandOutput) -> ToolStatus {
    ToolStatus {
        name: tool.name.to_owned(),
        found: true,
        required: tool.required,
        path: Some(out.stdout.trim().to_owned()),
    }
}

/// Build a missing-tool result.
fn missing_status(tool: &ToolProbe) -> ToolStatus {
    ToolStatus {
        name: tool.name.to_owned(),
        found: false,
        required: tool.required,
        path: None,
    }
}

/// Render results in the requested format.
fn render_results(results: &[ToolStatus], format: &OutputFormat, writer: &mut dyn Write) -> Result<(), ForgeError> {
    match format {
        OutputFormat::Json => render_json(results, writer),
        OutputFormat::Text => render_text(results, writer),
    }
}

/// Render results as JSON.
///
/// The payload carries an overall `healthy` flag, and the envelope
/// status is `Error` when required tools are missing so downstream
/// parsers see the failure without inspecting individual tools.
fn render_json(results: &[ToolStatus], writer: &mut dyn Write) -> Result<(), ForgeError> {
    let healthy = missing_tools(results).is_empty();
    let mut envelope = output::success(serde_json::json!({ "tools": results, "healthy": healthy }));
    if !healthy {
        envelope.status = "Error";
    }
    output::write_json(writer, &envelope)?;
    Ok(())
}

/// Render results as human-readable text.
fn render_text(results: &[ToolStatus], writer: &mut dyn Write) -> Result<(), ForgeError> {
    for tool in results {
        let icon = if tool.found { "ok" } else { "MISSING" };
        let req = if tool.required { " (required)" } else { "" };
        let path = tool
            .path
            .as_deref()
            .map(|path| format!(" -> {path}"))
            .unwrap_or_default();
        output::write_text(writer, &format!("  {icon}: {}{req}{path}", tool.name))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::runner::MockRunner;

    /// A successful `which` response.
    fn which_ok() -> CommandOutput {
        CommandOutput {
            status: 0,
            stdout: "/usr/bin/tool\n".to_owned(),
            stderr: String::new(),
        }
    }

    /// Build a mock runner where the given tools are on `PATH`.
    fn mock_with(tools: &[&str]) -> MockRunner {
        let mut runner = MockRunner::new();
        for tool in tools {
            runner.respond(&format!("which {tool}"), which_ok());
        }
        runner
    }

    /// Build a mock runner with a healthy tool set.
    fn mock_with_tools() -> MockRunner {
        mock_with(&["kubectl", "kind", "docker"])
    }

    /// Parse a JSON buffer, aborting on failure.
    fn parse_json(buf: &[u8]) -> serde_json::Value {
        serde_json::from_slice(buf).unwrap_or_else(|_| {
            std::process::abort();
            #[expect(unreachable_code, reason = "abort prevents reaching this")]
            {
                serde_json::Value::Null
            }
        })
    }

    #[test]
    fn doctor_reports_found_and_missing_tools() {
        let runner = mock_with_tools();
        let mut buf = Vec::new();
        run(&runner, &OutputFormat::Text, &mut buf).unwrap_or_else(|_| std::process::abort());
        let text = String::from_utf8_lossy(&buf);
        assert!(text.contains("ok: kubectl"), "kubectl should be found: {text}");
        assert!(text.contains("MISSING: podman"), "podman should be missing: {text}");
    }

    #[test]
    fn doctor_json_output_has_tools_array() {
        let runner = mock_with_tools();
        let mut buf = Vec::new();
        run(&runner, &OutputFormat::Json, &mut buf).unwrap_or_else(|_| std::process::abort());
        let parsed = parse_json(&buf);
        assert!(
            parsed
                .get("data")
                .and_then(|data| data.get("tools"))
                .and_then(|tools| tools.as_array())
                .is_some(),
            "should have data.tools array"
        );
        assert_eq!(
            parsed.get("data").and_then(|data| data.get("healthy")),
            Some(&serde_json::Value::Bool(true)),
            "a healthy tool set should report healthy: true"
        );
        assert_eq!(
            parsed.get("status").and_then(serde_json::Value::as_str),
            Some("Success"),
            "a healthy tool set should report status Success"
        );
    }

    #[test]
    fn doctor_fails_when_required_tool_missing() {
        let runner = mock_with(&["kubectl", "docker"]);
        let mut buf = Vec::new();
        let Err(err) = run(&runner, &OutputFormat::Text, &mut buf) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("kind"), "error should name the missing tool: {msg}");
        let text = String::from_utf8_lossy(&buf);
        assert!(
            text.contains("MISSING: kind (required)"),
            "report should still render: {text}"
        );
    }

    #[test]
    fn doctor_fails_without_container_runtime() {
        let runner = mock_with(&["kubectl", "kind"]);
        let mut buf = Vec::new();
        let Err(err) = run(&runner, &OutputFormat::Text, &mut buf) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(
            msg.contains("docker or podman"),
            "error should name the runtime group: {msg}"
        );
    }

    #[test]
    fn doctor_json_reports_unhealthy_as_error() {
        let runner = mock_with(&["docker"]);
        let mut buf = Vec::new();
        let result = run(&runner, &OutputFormat::Json, &mut buf);
        assert!(result.is_err(), "missing required tools must fail the run");
        let parsed = parse_json(&buf);
        assert_eq!(
            parsed.get("data").and_then(|data| data.get("healthy")),
            Some(&serde_json::Value::Bool(false)),
            "missing required tools should report healthy: false"
        );
        assert_eq!(
            parsed.get("status").and_then(serde_json::Value::as_str),
            Some("Error"),
            "missing required tools should report status Error"
        );
    }
}
