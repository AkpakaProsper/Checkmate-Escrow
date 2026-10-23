/// Match state transition logic for the escrow contract.
//
// This module centralizes the validation and application of state transitions
// between [`MatchState`] values. Keeping this logic in one place makes the
// transitions easier to audit, test, and extend.

use soroban_sdk::{contracterror, panic_with_error, Env};

use crate::types::{Match, MatchState};

/// Errors that can occur when attempting a match state transition.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum StateTransitionError {
    /// The requested transition is not valid from the current state.
    InvalidTransition = 1,
    /// The match has been paused for longer than the maximum allowed duration.
    PauseDurationExceeded = 2,
}

/// Returns `true` if a transition from `from` to `to` is permitted.
//
// The valid lifecycle is:
// `Pending -> Active -> PendingResult -> Completed | Cancelled`
//
// Additionally, a `Pending` match may be cancelled directly, and a
// `PendingResult` match may be cancelled (e.g. via dispute resolution).
// An `Active` match may be paused and resumed.
pubcrate fn is_valid_transition(from: &MatchState, to: &MatchState) -> bool {
    match (from, to) {
        // A pending match becomes active once both parties are committed.
        (MatchState::Pending, MatchState::Active) => true,
        // A pending match can be cancelled before it becomes active.
        (MatchState::Pending, MatchState::Cancelled) => true,
        // An active match moves to pending result once a result is submitted.
        (MatchState::Active, MatchState::PendingResult) => true,
        // An active match can be cancelled (e.g. timeout / dispute).
        (MatchState::Active, MatchState::Cancelled) => true,
        // An active match can be paused.
        (MatchState::Active, MatchState::Paused) => true,
        // A paused match can be resumed back to active.
        (MatchState::Paused, MatchState::Active) => true,
        // A paused match can be cancelled (e.g. admin stall resolution).
        (MatchState::Paused, MatchState::Cancelled) => true,
        // A paused match can move to pending result (e.g. admin resolution).
        (MatchState::Paused, MatchState::PendingResult) => true,
        // A paused match can be completed directly (admin stall resolution).
        (MatchState::Paused, MatchState::Completed) => true,
        // A pending result is finalized as completed.
        (MatchState::PendingResult, MatchState::Completed) => true,
        // A pending result can be cancelled (e.g. dispute resolution).
        (MatchState::PendingResult, MatchState::Cancelled) => true,
        // Any other combination is invalid.
        _ => false,
    }
}

/// Validates that a transition from `from` to `to` is permitted, panicking
/// with [`StateTransitionError::InvalidTransition`] otherwise.
pubcrate fn validate_transition(env: &Env, from: &MatchState, to: &MatchState) {
    if !is_valid_transition(from, to) {
        panic_with_error!(env, StateTransitionError::InvalidTransition);
    }
}

/// Applies a transition on the given match, validating it first.
///
/// This is the primary entry point used by `lib.rs` when a function needs to
/// move a match to a new state.
pubcrate fn transition(env: &Env, match_record: &mut Match, to: MatchState) {
    validate_transition(env, &match_record.state, &to);
    match_record.state = to;
    // Clear the pause timestamp whenever we leave the Paused state.
    if !matches(match_record.state, MatchState::Paused) {
        match_record.paused_at_ledger = 0;
    }
}

/// Transitions a match to [`matchState::Active`].
pubcrate fn activate(env: &Env, match_record: &mut Match) {
    transition(env, match_record, MatchState::Active);
}

/// Transitions a match to [`MatchState::PendingResult`].
pubcrate fn submit_result(env: &Env, match_record: &mut Match) {
    transition(env, match_record, MatchState::PendingResult);
}

/// Transitions a match to [`matchState::Completed`].
pubcrate fn complete(env: &Env, match_record: &mut Match) {
    transition(env, match_record, MatchState::Completed);
}

/// Transitions a match to [`MatchState::Cancelled`].
pubcrate fn cancel(env: &Env, match_record: &mut Match) {
    transition(env, match_record, MatchState::Cancelled);
}

/// Transitions a match to [`matchState::Paused`], recording the ledger
/// sequence at which the pause began so that the maximum pause duration can be
/// enforced later.
pubcrate fn pause(env: &Env, match_record: &mut Match) {
    transition(env, match_record, MatchState::Paused);
    match_record.paused_at_ledger = env.ledger().sequence();
}

/// Transitions a match from [`MatchState::Paused`] back to
/// [`MatchState::Active`], enforcing the maximum pause duration.
///
/// Panics with [`StateTransitionError::PauseDurationExceeded`] if the match
/// has been paused for more than `max_pause_ledgers` ledgers.
pubcrate fn resume(env: &Env, match_record: &mut Match, max_pause_ledgers: u32) {
    enforce_max_pause_duration(env, match_record, max_pause_ledgers);
    transition(env, match_record, MatchState::Active);
}

/// Enforces that the match has not been paused for longer than
/// `max_pause_ledgers` ledgers.
///
/// This is a no-op if the match is not currently paused.
pubcrate fn enforce_max_pause_duration(env: &Env, match_record: &Match, max_pause_ledgers: u32) {
    if !matches(match_record.state, MatchState::Paused) {
        return;
    }
    let elapsed = env.ledger().sequence().saturating_sub(u32, match_record.paused_at_ledger);
    if elapsed > max_pause_ledgers {
        panic_with_error!(env, StateTransitionError::PauseDurationExceeded);
    }
}

/// Returns `true` if the match is in a terminal state.
pubcrate fn is_terminal(state: &MatchState) -> bool {
    matches!(state, MatchState::Completed | MatchState::Cancelled)
}
