//! Tests for issue #1763:
//! A paused contract must reject every state-changing call with `Error::ContractPaused`.
//!
//! This file systematically asserts that after `pause()`:
//!   - `create_match`
//!   - `deposit`
//!   - `submit_result`
//!   - `cancel_match`
//!
//! all return `Err(Error::ContractPaused)`, and that after `unpause()` those
//! same calls succeed (or at least do not fail with `ContractPaused`).
//!
//! Background: The oracle's `submit_result` intentionally bypasses the pause
//! flag so it can settle already-active matches whose payout is overdue; the
//! contract-level pause check in the *public* `submit_result` entry-point
//! enforces the guard before the oracle auth check, ensuring unprivileged
//! callers still see `ContractPaused`.

use super::*;

// ─────────────────────────────────────────────────────────────────────────────
// create_match is rejected while paused
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_1763_create_match_blocked_while_paused() {
    let (env, contract_id, _oracle, player1, player2, token, admin) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    client.pause(&admin);

    let result = client.try_create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "1763a001"),
        &Platform::Lichess,
    );

    assert_eq!(
        result,
        Err(Ok(Error::ContractPaused)),
        "create_match must return ContractPaused when the contract is paused"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// deposit is rejected while paused
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_1763_deposit_blocked_while_paused() {
    let (env, contract_id, _oracle, player1, player2, token, admin) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // Create a match first, before pausing.
    let match_id = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "1763b001"),
        &Platform::Lichess,
    );

    client.pause(&admin);

    let result = client.try_deposit(&match_id, &player1);

    assert_eq!(
        result,
        Err(Ok(Error::ContractPaused)),
        "deposit must return ContractPaused when the contract is paused"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// submit_result is rejected while paused
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_1763_submit_result_blocked_while_paused() {
    let (env, contract_id, oracle, player1, player2, token, admin) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // Create + fully fund a match before pausing.
    let match_id = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "1763c001"),
        &Platform::Lichess,
    );
    client.deposit(&match_id, &player1);
    client.deposit(&match_id, &player2);

    client.pause(&admin);

    let result = client.try_submit_result(&match_id, &Winner::Player1, &oracle, &None);

    assert_eq!(
        result,
        Err(Ok(Error::ContractPaused)),
        "submit_result must return ContractPaused when the contract is paused"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// cancel_match is rejected while paused
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_1763_cancel_match_blocked_while_paused() {
    let (env, contract_id, _oracle, player1, player2, token, admin) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // Create a pending match, then pause before any deposits.
    let match_id = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "1763d001"),
        &Platform::Lichess,
    );

    client.pause(&admin);

    let result = client.try_cancel_match(&match_id, &player1);

    assert_eq!(
        result,
        Err(Ok(Error::ContractPaused)),
        "cancel_match must return ContractPaused when the contract is paused"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// After unpause all four operations succeed (or fail for a non-pause reason)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_1763_create_match_succeeds_after_unpause() {
    let (env, contract_id, _oracle, player1, player2, token, admin) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    client.pause(&admin);
    client.unpause(&admin);

    let result = client.try_create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "1763e001"),
        &Platform::Lichess,
    );

    assert!(
        result.is_ok(),
        "create_match must succeed after the contract is unpaused"
    );
}

#[test]
fn test_1763_deposit_succeeds_after_unpause() {
    let (env, contract_id, _oracle, player1, player2, token, admin) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let match_id = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "1763f001"),
        &Platform::Lichess,
    );

    client.pause(&admin);
    client.unpause(&admin);

    // Should succeed — not fail with ContractPaused.
    let result = client.try_deposit(&match_id, &player1);
    assert!(
        result.is_ok(),
        "deposit must succeed after the contract is unpaused"
    );
}

#[test]
fn test_1763_cancel_match_succeeds_after_unpause() {
    let (env, contract_id, _oracle, player1, player2, token, admin) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let match_id = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "1763g001"),
        &Platform::Lichess,
    );

    client.pause(&admin);
    client.unpause(&admin);

    // Cancelling a Pending (unfunded) match should succeed.
    let result = client.try_cancel_match(&match_id, &player1);
    assert!(
        result.is_ok(),
        "cancel_match must succeed after the contract is unpaused"
    );
}

#[test]
fn test_1763_submit_result_succeeds_after_unpause() {
    let (env, contract_id, oracle, player1, player2, token, admin) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let match_id = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "1763h001"),
        &Platform::Lichess,
    );
    client.deposit(&match_id, &player1);
    client.deposit(&match_id, &player2);

    client.pause(&admin);
    client.unpause(&admin);

    let result = client.try_submit_result(&match_id, &Winner::Player1, &oracle, &None);
    assert!(
        result.is_ok(),
        "submit_result must succeed after the contract is unpaused"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Pause/unpause are themselves idempotent-safe under test conditions
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_1763_pause_then_unpause_cycle_does_not_corrupt_state() {
    let (env, contract_id, oracle, player1, player2, token, admin) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // Full match lifecycle with a pause/unpause in the middle.
    let match_id = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "1763i001"),
        &Platform::Lichess,
    );
    client.deposit(&match_id, &player1);

    // Pause mid-deposit.
    client.pause(&admin);

    // player2 cannot deposit while paused.
    let blocked = client.try_deposit(&match_id, &player2);
    assert_eq!(
        blocked,
        Err(Ok(Error::ContractPaused)),
        "deposit must be blocked while paused mid-match"
    );

    // Unpause and complete the lifecycle.
    client.unpause(&admin);
    client.deposit(&match_id, &player2);
    let settle = client.try_submit_result(&match_id, &Winner::Player2, &oracle, &None);
    assert!(
        settle.is_ok(),
        "match should settle normally after pause/unpause cycle"
    );
}
