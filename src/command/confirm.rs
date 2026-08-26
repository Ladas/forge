//! Interactive confirmation for destructive commands.
//!
//! `down` and `cluster delete` ask for confirmation before deleting
//! resources, but only when standard input is an interactive
//! terminal.  Non-TTY invocations (scripts, CI) never prompt and
//! proceed unchanged, and `--force` or the global `--non-interactive`
//! flag skips the prompt on a terminal.

use std::io::{BufRead as _, IsTerminal as _, Write};

use crate::{
    error::ForgeError,
    output::{self, OutputFormat},
};

/// Ask the user to confirm a destructive action.
///
/// Returns `true` when the action should proceed: the prompt was
/// skipped (`skip_prompt`, or stdin is not a terminal) or the user
/// answered yes.
///
/// # Errors
///
/// Returns [`ForgeError`] if reading the answer fails.
#[expect(clippy::print_stderr, reason = "interactive prompt on the controlling terminal")]
#[expect(
    clippy::disallowed_methods,
    reason = "the one sanctioned interactive read: automated mode (non-TTY \
              stdin, --force, or --non-interactive) never reaches it"
)]
pub fn confirm_destructive(action: &str, skip_prompt: bool) -> Result<bool, ForgeError> {
    let stdin = std::io::stdin();
    if skip_prompt || !stdin.is_terminal() {
        return Ok(true);
    }
    eprint!("{action}? [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    stdin.lock().read_line(&mut answer)?;
    Ok(is_affirmative(&answer))
}

/// True when a prompt answer means yes.
fn is_affirmative(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Report that the user declined the confirmation prompt.
///
/// # Errors
///
/// Returns [`ForgeError`] if writing the report fails.
pub fn report_declined(writer: &mut dyn Write, format: &OutputFormat) -> Result<(), ForgeError> {
    match format {
        OutputFormat::Json => {
            let result = output::success(serde_json::json!({ "aborted": true }));
            output::write_json(writer, &result)?;
        },
        OutputFormat::Text => output::write_text(writer, "aborted: confirmation declined")?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn affirmative_answers_are_recognized() {
        for answer in ["y", "Y", "yes", "YES", " yes \n"] {
            assert!(is_affirmative(answer), "{answer:?} should confirm");
        }
    }

    #[test]
    fn non_affirmative_answers_decline() {
        for answer in ["", "n", "no", "maybe", "yess"] {
            assert!(!is_affirmative(answer), "{answer:?} should decline");
        }
    }

    #[test]
    fn skip_prompt_always_proceeds() {
        let proceed = confirm_destructive("delete everything", true).unwrap_or_else(|_| std::process::abort());
        assert!(proceed, "skip_prompt must bypass the prompt");
    }

    #[test]
    fn non_tty_stdin_proceeds_without_prompting() {
        // Under the test harness stdin is not a terminal, so the
        // prompt must be skipped and the action allowed; this is the
        // automation-preserving contract for scripts and CI.
        let proceed = confirm_destructive("delete everything", false).unwrap_or_else(|_| std::process::abort());
        assert!(proceed, "a non-TTY stdin must never block on a prompt");
    }

    #[test]
    fn declined_report_renders_text_and_json() {
        let mut text_buf = Vec::new();
        report_declined(&mut text_buf, &OutputFormat::Text).unwrap_or_else(|_| std::process::abort());
        let text = String::from_utf8_lossy(&text_buf);
        assert!(text.contains("aborted"), "text report should say aborted: {text}");

        let mut json_buf = Vec::new();
        report_declined(&mut json_buf, &OutputFormat::Json).unwrap_or_else(|_| std::process::abort());
        let parsed: serde_json::Value = serde_json::from_slice(&json_buf).unwrap_or_else(|_| {
            std::process::abort();
            #[expect(unreachable_code, reason = "abort prevents reaching this")]
            {
                serde_json::Value::Null
            }
        });
        assert_eq!(
            parsed.get("data").and_then(|data| data.get("aborted")),
            Some(&serde_json::Value::Bool(true)),
            "JSON report should carry aborted: true"
        );
    }
}
