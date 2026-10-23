//! Tests for Issue #1814 — Fix: rotate_oracle_temporary does not restore the
//! original oracle when the duration expires.
//!
//! `rotate_oracle_temporary` stores the temporary oracle and an expiry
//! timestamp, but provides no on-chain trigger to restore the original oracle
//! once the window closes.  Without intervention the temporary oracle stays
//! authoritative indefinitely.
//!
//! The fix adds `finalize_temp_oracle_rotation`, callable by anyone after
//! expiry, which clears the stale rotation record and emits `admin/rot_restored`.

use super::*;

/// Advance the ledger timestamp by `seconds`.
fn advance_timestamp(env: &Env, seconds: u64) {
    let current = env.ledger().timestamp();
    env.ledger()
        .set_timestamp(current.saturating_add(seconds));
}

// ── Happy-path: successful finalization ───────────────────────────────────────

/// After the rotation window elapses, `finalize_temp_oracle_rotation` must
/// succeed and `get_oracle` must return the original oracle.
#[test]
fn test_finalize_temp_oracle_rotation_restores_original_oracle() {
    let (env, contract_id, oracle, _player1, _player2, _token, _admin) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    env.ledger().set_timestamp(1_000);

    let temp_oracle = Address::generate(&env);
    let duration: u64 = 3600; // minimum allowed (1 hour)

    client.rotate_oracle_temporary(&oracle, &temp_oracle, &duration);

    // Within the window: effective oracle is the temp one.
    assert_eq!(client.get_oracle(), temp_oracle);

    // Advance past the expiry.
    advance_timestamp(&env, duration);

    // Anyone can call finalize — no auth needed.
    client.finalize_temp_oracle_rotation();

    // Original oracle must now be active again.
    assert_eq!(
        client.get_oracle(),
        oracle,
        "original oracle must be restored after finalization"
    );
}

/// `finalize_temp_oracle_rotation` is callable by a third party (not the admin
/// or either oracle) once the rotation window has closed.
#[test]
fn test_finalize_temp_oracle_rotation_callable_by_anyone() {
    let (env, contract_id, oracle, player1, _player2, _token, _admin) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    env.ledger().set_timestamp(1_000);

    let temp_oracle = Address::generate(&env);
    client.rotate_oracle_temporary(&oracle, &temp_oracle, &3600u64);

    advance_timestamp(&env, 3600);

    // player1 has no special role — finalize must still succeed.
    let _ = player1; // just to show intent; mock_all_auths handles the call
    client.finalize_temp_oracle_rotation();

    assert_eq!(client.get_oracle(), oracle);
}

/// Rotation state is cleared after finalization; a second call must return
/// `InvalidState` because there is no longer a recorded temp rotation.
#[test]
fn test_finalize_temp_oracle_rotation_clears_state() {
    let (env, contract_id, oracle, _player1, _player2, _token, _admin) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    env.ledger().set_timestamp(1_000);

    let temp_oracle = Address::generate(&env);
    client.rotate_oracle_temporary(&oracle, &temp_oracle, &3600u64);

    advance_timestamp(&env, 3600);

    client.finalize_temp_oracle_rotation();

    // A second call finds no temp rotation and must fail with InvalidState.
    let result = client.try_finalize_temp_oracle_rotation();
    assert_eq!(
        result,
        Err(Ok(Error::InvalidState)),
        "second finalize call must return InvalidState — rotation already cleared"
    );
}

// ── Early-call rejection ──────────────────────────────────────────────────────

/// Calling `finalize_temp_oracle_rotation` *before* expiry must fail with
/// `InvalidState`.
#[test]
fn test_finalize_temp_oracle_rotation_rejects_before_expiry() {
    let (env, contract_id, oracle, _player1, _player2, _token, _admin) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    env.ledger().set_timestamp(1_000);

    let temp_oracle = Address::generate(&env);
    let duration: u64 = 3600;
    client.rotate_oracle_temporary(&oracle, &temp_oracle, &duration);

    // Advance to one second before expiry.
    advance_timestamp(&env, duration - 1);

    let result = client.try_finalize_temp_oracle_rotation();
    assert_eq!(
        result,
        Err(Ok(Error::InvalidState)),
        "finalize must be rejected when the rotation window has not yet elapsed"
    );

    // Effective oracle must still be the temporary one.
    assert_eq!(client.get_oracle(), temp_oracle);
}

/// Calling `finalize_temp_oracle_rotation` with no active temp rotation (i.e.
/// before any `rotate_oracle_temporary` is called) must fail with `InvalidState`.
#[test]
fn test_finalize_temp_oracle_rotation_rejects_with_no_rotation() {
    let (env, contract_id, _oracle, _player1, _player2, _token, _admin) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let result = client.try_finalize_temp_oracle_rotation();
    assert_eq!(
        result,
        Err(Ok(Error::InvalidState)),
        "finalize must return InvalidState when no temp rotation exists"
    );
}

// ── Event emission ────────────────────────────────────────────────────────────

/// `finalize_temp_oracle_rotation` must emit an `admin/rot_restored` event
/// carrying `(temp_oracle, restored_oracle)`.
#[test]
fn test_finalize_temp_oracle_rotation_emits_rot_restored_event() {
    let (env, contract_id, oracle, _player1, _player2, _token, _admin) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    env.ledger().set_timestamp(1_000);

    let temp_oracle = Address::generate(&env);
    client.rotate_oracle_temporary(&oracle, &temp_oracle, &3600u64);

    advance_timestamp(&env, 3600);

    client.finalize_temp_oracle_rotation();

    // Locate the admin/rot_restored event.
    let all_events = env.events().all();
    let expected_topics = vec![
        &env,
        Symbol::new(&env, "admin").into_val(&env),
        symbol_short!("rot_rest").into_val(&env),
    ];
    let matched = all_events
        .iter()
        .find(|(_, topics, _)| *topics == expected_topics);
    assert!(matched.is_some(), "admin/rot_restored event must be emitted");

    // Payload: (temp_oracle, original_oracle)
    let (_, _, data) = matched.unwrap();
    let (emitted_temp, emitted_original): (Address, Address) =
        TryFromVal::try_from_val(&env, &data)
            .expect("event payload must deserialize as (Address, Address)");

    assert_eq!(emitted_temp, temp_oracle, "first payload field must be the temp oracle");
    assert_eq!(
        emitted_original, oracle,
        "second payload field must be the restored (original) oracle"
    );
}

// ── Boundary: finalize at exactly expiry ─────────────────────────────────────

/// `finalize_temp_oracle_rotation` must succeed when called at exactly
/// `timestamp == expiry` (the boundary condition is `now >= expiry`).
#[test]
fn test_finalize_temp_oracle_rotation_succeeds_at_exact_expiry() {
    let (env, contract_id, oracle, _player1, _player2, _token, _admin) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let start: u64 = 1_000;
    let duration: u64 = 3600;
    env.ledger().set_timestamp(start);

    let temp_oracle = Address::generate(&env);
    client.rotate_oracle_temporary(&oracle, &temp_oracle, &duration);

    // Move to exactly the expiry second.
    env.ledger().set_timestamp(start + duration);

    client.finalize_temp_oracle_rotation();

    assert_eq!(client.get_oracle(), oracle);
}
