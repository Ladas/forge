//! Entry point for the `praxis-forge` CLI.

use clap::Parser as _;
use forge::{
    cli::{Cli, ClusterCommand, Command, ConfigCommand, ServiceCommand, StackCommand},
    cluster,
    command::{config, doctor, down, plan, runner, status, up},
    context::ForgeContext,
    error::ForgeError,
    output::{self, OutputFormat},
    stack,
};

/// Parse CLI arguments and dispatch to the appropriate handler.
fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let mut stdout = std::io::stdout();
    let result = dispatch(&cli, &mut stdout);
    let format = error_format(&cli);
    handle_result(result, &format)
}

/// Dispatch the parsed command to its handler.
fn dispatch(cli: &Cli, writer: &mut dyn std::io::Write) -> Result<(), ForgeError> {
    let format = &cli.global.output;
    match &cli.command {
        Command::Doctor => dispatch_doctor(format, writer),
        Command::Plan => dispatch_plan(cli, format, writer),
        Command::Config(sub) => dispatch_config(cli, sub, format, writer),
        Command::Up => dispatch_up(cli, writer),
        Command::Down { force } => dispatch_down(cli, *force, writer),
        Command::Status { json } => dispatch_status(cli, *json, writer),
        Command::Apply { cluster, stack } => {
            let sub = StackCommand::Apply {
                cluster: cluster.clone(),
                stack: stack.clone(),
            };
            dispatch_stack(cli, &sub, writer)
        },
        Command::Cluster(sub) => dispatch_cluster(cli, sub, writer),
        Command::Service(sub) => dispatch_service(cli, sub, writer),
        Command::Stack(sub) => dispatch_stack(cli, sub, writer),
    }
}

/// Run the doctor command.
fn dispatch_doctor(format: &OutputFormat, writer: &mut dyn std::io::Write) -> Result<(), ForgeError> {
    let runner = runner::SystemRunner;
    doctor::run(&runner, format, writer)
}

/// Run the plan command.
fn dispatch_plan(cli: &Cli, format: &OutputFormat, writer: &mut dyn std::io::Write) -> Result<(), ForgeError> {
    plan::run(&cli.global.config, format, writer)
}

/// Dispatch config subcommands.
fn dispatch_config(
    cli: &Cli,
    sub: &ConfigCommand,
    format: &OutputFormat,
    writer: &mut dyn std::io::Write,
) -> Result<(), ForgeError> {
    match sub {
        ConfigCommand::Validate => config::run_validate(&cli.global.config, format, writer),
        ConfigCommand::Show { resolved } => config::run_show(&cli.global.config, *resolved, format, writer),
        ConfigCommand::Init { force } => {
            config::run_init(&cli.global.config, *force, cli.global.dry_run, format, writer)
        },
        ConfigCommand::Schema => config::run_schema(writer),
    }
}

/// Load config and validate it.
fn load_config_validated(cli: &Cli) -> Result<forge::config::ForgeConfig, ForgeError> {
    let mut cfg = forge::config::load(&cli.global.config)?;
    if let Some(runtime) = &cli.global.runtime {
        cfg.spec.runtime.provider = runtime.clone();
    }
    forge::config::validate::validate(&cfg)?;
    Ok(cfg)
}

/// Build a [`ForgeContext`] from CLI options.
fn build_context<'ctx>(
    cli: &'ctx Cli,
    runner: &'ctx dyn runner::CommandRunner,
    config: &'ctx forge::config::ForgeConfig,
) -> Result<ForgeContext<'ctx>, ForgeError> {
    Ok(ForgeContext {
        runner,
        config,
        state_dir: cli.global.state_dir.clone(),
        config_dir: config_dir_from_path(&cli.global.config)?,
        format: cli.global.output.clone(),
        dry_run: cli.global.dry_run,
    })
}

/// Derive the config directory from the config file path.
///
/// Canonicalizes the result so that Docker volume bind-mounts and
/// containment checks receive absolute paths instead of relative
/// ones.  A bare filename (the default `forge.yaml`) has an empty
/// parent, which is treated as the current directory.  A parent
/// that cannot be canonicalized is an error: falling back to a
/// relative or empty base would silently disable the volume
/// containment check downstream.
fn config_dir_from_path(path: &std::path::Path) -> Result<std::path::PathBuf, ForgeError> {
    let parent = match path.parent() {
        Some(par) if !par.as_os_str().is_empty() => par.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    };
    std::fs::canonicalize(&parent)
        .map_err(|err| ForgeError::Config(format!("config directory {}: {err}", parent.display())))
}

/// Dispatch the `up` command.
fn dispatch_up(cli: &Cli, writer: &mut dyn std::io::Write) -> Result<(), ForgeError> {
    let config = load_config_validated(cli)?;
    let runner = runner::SystemRunner;
    let ctx = build_context(cli, &runner, &config)?;
    up::run(&ctx, writer)
}

/// Dispatch the `down` command.
fn dispatch_down(cli: &Cli, force: bool, writer: &mut dyn std::io::Write) -> Result<(), ForgeError> {
    let config = load_config_validated(cli)?;
    let runner = runner::SystemRunner;
    let ctx = build_context(cli, &runner, &config)?;
    down::run(&ctx, force, writer)
}

/// Dispatch the `status` command.
fn dispatch_status(cli: &Cli, json_flag: bool, writer: &mut dyn std::io::Write) -> Result<(), ForgeError> {
    let config = load_config_validated(cli)?;
    let runner = runner::SystemRunner;
    let mut ctx = build_context(cli, &runner, &config)?;
    if json_flag {
        ctx.format = OutputFormat::Json;
    }
    status::run(&ctx, writer)
}

/// Dispatch service subcommands.
fn dispatch_service(cli: &Cli, sub: &ServiceCommand, writer: &mut dyn std::io::Write) -> Result<(), ForgeError> {
    let config = load_config_validated(cli)?;
    let runner = runner::SystemRunner;
    let ctx = build_context(cli, &runner, &config)?;
    forge::service::dispatch(&ctx, sub, writer)
}

/// Dispatch stack subcommands.
fn dispatch_stack(cli: &Cli, sub: &StackCommand, writer: &mut dyn std::io::Write) -> Result<(), ForgeError> {
    let config = load_config_validated(cli)?;
    let runner = runner::SystemRunner;
    let ctx = build_context(cli, &runner, &config)?;
    stack::dispatch(&ctx, sub, writer)
}

/// Dispatch cluster subcommands.
fn dispatch_cluster(cli: &Cli, sub: &ClusterCommand, writer: &mut dyn std::io::Write) -> Result<(), ForgeError> {
    let config = load_config_validated(cli)?;
    let runner = runner::SystemRunner;
    let ctx = build_context(cli, &runner, &config)?;
    cluster::dispatch(&ctx, sub, writer)
}

/// Handle the result of command dispatch.
fn handle_result(result: Result<(), ForgeError>, format: &OutputFormat) -> std::process::ExitCode {
    let Err(err) = result else {
        return std::process::ExitCode::SUCCESS;
    };
    report_error(&err, format);
    std::process::ExitCode::FAILURE
}

/// Resolve the output format used for top-level error reporting.
fn error_format(cli: &Cli) -> OutputFormat {
    if matches!(cli.command, Command::Status { json: true }) {
        OutputFormat::Json
    } else {
        cli.global.output.clone()
    }
}

/// Print an error to stderr in the appropriate format.
#[expect(clippy::print_stderr, reason = "CLI error reporting")]
fn report_error(err: &ForgeError, format: &OutputFormat) {
    match format {
        OutputFormat::Json => {
            let envelope = output::error(&err.to_string());
            let json = serde_json::to_string_pretty(&envelope)
                .unwrap_or_else(|_| r#"{"error":"serialization failed"}"#.to_owned());
            eprintln!("{json}");
        },
        OutputFormat::Text => {
            eprintln!("error: {err}");
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_dir_for_bare_filename_is_current_dir() {
        let dir = config_dir_from_path(std::path::Path::new("forge.yaml")).unwrap_or_else(|_| std::process::abort());
        let expected = std::fs::canonicalize(".").unwrap_or_else(|_| std::process::abort());
        assert!(dir.is_absolute(), "config dir must be absolute, got: {}", dir.display());
        assert_eq!(dir, expected, "bare filename must resolve to the current directory");
    }

    #[test]
    fn config_dir_for_relative_path_is_canonicalized() {
        let dir = config_dir_from_path(std::path::Path::new("./forge.yaml")).unwrap_or_else(|_| std::process::abort());
        let expected = std::fs::canonicalize(".").unwrap_or_else(|_| std::process::abort());
        assert_eq!(
            dir, expected,
            "relative path must canonicalize to the current directory"
        );
    }

    #[test]
    fn config_dir_errors_for_missing_parent() {
        let missing = std::path::Path::new("/nonexistent-forge-test-dir/forge.yaml");
        let Err(err) = config_dir_from_path(missing) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(
            msg.contains("config directory"),
            "expected a config-directory error, got: {msg}"
        );
    }
}
