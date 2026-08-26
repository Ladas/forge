//! Shared state-checkpointing helpers for mutating commands.
//!
//! `up` and `down` both run in phases that create or destroy real
//! resources. These helpers persist state after every phase so that
//! whatever a phase recorded survives a later failure.

use crate::{context::ForgeContext, error::ForgeError, state};

/// Persist the working state, unless this is a dry run.
///
/// # Errors
///
/// Returns [`ForgeError`] if the state file cannot be written.
pub fn checkpoint(ctx: &ForgeContext<'_>, state: &state::ForgeState) -> Result<(), ForgeError> {
    if ctx.dry_run {
        return Ok(());
    }
    state::save(&ctx.state_dir, state)
}

/// Run one phase of a mutating command and persist whatever it
/// recorded, pass or fail.
///
/// Each phase creates or removes real resources — a network, KIND
/// clusters, containers — before it can fail. Persisting only after
/// every phase succeeded would leave state out of step with reality
/// whenever a later phase fails: resources `up` created would be
/// unrecorded (and `forge down` acts solely on what state records, so
/// they would be orphaned with no supported way to remove them), and
/// resources `down` already removed would still be reported as
/// running.
///
/// # Errors
///
/// Returns the phase's error if it failed, otherwise any checkpoint error.
pub fn checkpointed<T, Phase>(
    ctx: &ForgeContext<'_>,
    state: &mut state::ForgeState,
    phase: Phase,
) -> Result<T, ForgeError>
where
    Phase: FnOnce(&mut state::ForgeState) -> Result<T, ForgeError>,
{
    let outcome = phase(state);
    let persisted = checkpoint(ctx, state);
    match (outcome, persisted) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(save_err)) => Err(save_err),
        (Err(phase_err), Ok(())) => Err(phase_err),
        // Both failing is the one case that must not be reported as either one
        // alone: the phase error is what stopped the run, but a lost checkpoint
        // means the resources it already created are unrecorded, and a user
        // told only "cluster creation failed" would expect `down` to clean up.
        (Err(phase_err), Err(save_err)) => Err(ForgeError::State(format!(
            "{phase_err}; the state file could not be written either, so \
             resources created before the failure are not recorded and \
             `forge down` will not remove them: {save_err}"
        ))),
    }
}

/// Record the last operation in state.
pub fn record_operation(state: &mut state::ForgeState, operation: &str, success: bool) {
    state.last_operation = Some(state::LastOperation {
        operation: operation.to_owned(),
        timestamp: state::now_epoch_secs(),
        success,
    });
}
