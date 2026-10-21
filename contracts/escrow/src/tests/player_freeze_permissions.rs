//! Tests for player-freeze permission boundaries.
//!
//! This module covers the access-control rules around the freeze/unfreeze
//! admin operations: only the configured admin may freeze or unfreeze players,
//! and frozen players cannot create matches or deposit — but can still cancel
//! pending matches and claim payouts.
//!
//! NOTE: Comprehensive freeze-guard tests live in `player_freeze.rs`. This
//! module focuses on permission/authorisation edge cases.

use super::*;

// ── Only admin may freeze ────────────────────────────────────────────────────

#[test]
fn test_freeze_permissions_only_admin_can_freeze() {
    let (env, contract_id, _oracle, player1, player2, _token, _admin) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // Attempt to freeze player2 as player1 (non-admin) — must be rejected.
    env.set_auths(&[]);
    let result = client.try_admin_freeze_player(
        &player2,
        &soroban_sdk::String::from_str(&env, "unauthorized"),
    );
    assert!(
        result.is_err(),
        "non-admin must not be able to freeze a player"
    );

    // player2 must still not be frozen.
    let _ = env.mock_all_auths();
    assert!(
        !client.is_player_frozen(&player2),
        "player2 must remain unfrozen after unauthorized freeze attempt"
    );

    let _ = player1; // suppress unused warning
}

// ── Only admin may unfreeze ──────────────────────────────────────────────────

#[test]
fn test_freeze_permissions_only_admin_can_unfreeze() {
    let (env, contract_id, _oracle, player1, player2, _token, _admin) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // Admin freezes player2.
    client.admin_freeze_player(
        &player2,
        &soroban_sdk::String::from_str(&env, "test"),
    );
    assert!(client.is_player_frozen(&player2));

    // Attempt to unfreeze as player1 (non-admin).
    env.set_auths(&[]);
    let result = client.try_admin_unfreeze_player(&player2);
    assert!(
        result.is_err(),
        "non-admin must not be able to unfreeze a player"
    );

    // player2 must still be frozen.
    let _ = env.mock_all_auths();
    assert!(
        client.is_player_frozen(&player2),
        "player2 must remain frozen after unauthorized unfreeze attempt"
    );

    let _ = player1;
}
