//! Regression tests for issue #1616: `finalize_match` and
//! `resolve_dispute_by_vote` must not enable a second payout when
//! `claim_vested_payout` is called after either of those settlement paths.
//!
//! Both functions call `execute_payout` which immediately transfers tokens out
//! of the contract.  A subsequent call to `claim_vested_payout` must not
//! succeed (the contract has already paid out the pot), and the player and
//! contract token balances must reflect exactly **one** payout.

use super::*;
use soroban_sdk::testutils::Ledger as _;

// ── shared helpers ────────────────────────────────────────────────────────────

/// Set up a match that has entered `PendingResult` via `submit_result` with a
/// non-zero dispute period.  Returns `(env, contract_id, oracle, player1, player2, token, match_id)`.
///
/// The caller is responsible for advancing the ledger past the dispute deadline
/// before calling `finalize_match`.
fn setup_pending_result(
    dispute_period: u32,
    game_id: &str,
) -> (Env, Address, Address, Address, Address, Address, u64) {
    let (env, contract_id, oracle, player1, player2, token, _admin) =
        setup_with_dispute_period(dispute_period);
    let client = EscrowContractClient::new(&env, &contract_id);

    let match_id = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, game_id),
        &Platform::Lichess,
    );
    client.deposit(&match_id, &player1);
    client.deposit(&match_id, &player2);

    // Settle at a known ledger so the deadline is predictable.
    env.ledger().set_sequence_number(1000);
    client.submit_result(&match_id, &Winner::Player1, &oracle);

    (env, contract_id, oracle, player1, player2, token, match_id)
}

// ── finalize_match ────────────────────────────────────────────────────────────

/// After `finalize_match` the winner has already received the pot.
/// A follow-up `claim_vested_payout` call must fail and balances must remain
/// unchanged (exactly one payout, not two).
#[test]
fn test_finalize_match_no_double_payout_for_winner() {
    let dispute_period: u32 = 100;
    let (env, contract_id, _oracle, player1, player2, token, match_id) =
        setup_pending_result(dispute_period, "dp1616a1");
    let client = EscrowContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    // Advance past the dispute deadline (1000 + 100 = 1100).
    env.ledger().set_sequence_number(1100);
    client.finalize_match(&match_id);

    // After finalize_match the pot (200) has been paid to player1.
    assert_eq!(
        token_client.balance(&player1), 1100,
        "player1 balance should be 1100 after one finalize_match payout"
    );
    assert_eq!(
        token_client.balance(&player2), 900,
        "player2 balance should be 900 (lost their stake)"
    );
    assert_eq!(
        token_client.balance(&contract_id), 0,
        "contract should hold 0 tokens after finalize_match"
    );

    // Attempting a second payout via claim_vested_payout must fail —
    // the contract has already disbursed all funds.
    let result = client.try_claim_vested_payout(&match_id, &player1);
    assert!(
        result.is_err(),
        "claim_vested_payout must fail after finalize_match has already paid out"
    );

    // Balances must be unchanged — still exactly one payout, not two.
    assert_eq!(
        token_client.balance(&player1), 1100,
        "player1 balance must not change after a rejected double-payout attempt"
    );
    assert_eq!(
        token_client.balance(&player2), 900,
        "player2 balance must not change after a rejected double-payout attempt"
    );
    assert_eq!(
        token_client.balance(&contract_id), 0,
        "contract balance must remain 0 after a rejected double-payout attempt"
    );
}

/// Same scenario with `Winner::Player2` to confirm the guard applies
/// regardless of which side won.
#[test]
fn test_finalize_match_no_double_payout_player2_win() {
    let (env, contract_id, oracle, player1, player2, token, _admin) =
        setup_with_dispute_period(100);
    let client = EscrowContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    let match_id = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "dp1616b1"),
        &Platform::Lichess,
    );
    client.deposit(&match_id, &player1);
    client.deposit(&match_id, &player2);

    env.ledger().set_sequence_number(1000);
    client.submit_result(&match_id, &Winner::Player2, &oracle);

    // Advance past the dispute deadline.
    env.ledger().set_sequence_number(1100);
    client.finalize_match(&match_id);

    // player2 wins the pot.
    assert_eq!(token_client.balance(&player1), 900);
    assert_eq!(token_client.balance(&player2), 1100);
    assert_eq!(token_client.balance(&contract_id), 0);

    // A claim attempt by player2 must fail.
    let result = client.try_claim_vested_payout(&match_id, &player2);
    assert!(
        result.is_err(),
        "claim_vested_payout must fail after finalize_match has already paid out"
    );

    // Balances unchanged — one payout only.
    assert_eq!(token_client.balance(&player1), 900);
    assert_eq!(token_client.balance(&player2), 1100);
    assert_eq!(token_client.balance(&contract_id), 0);
}

/// Draw: both players receive their stake back via `finalize_match`.
/// Neither player should be able to claim again via `claim_vested_payout`.
#[test]
fn test_finalize_match_no_double_payout_draw() {
    let (env, contract_id, oracle, player1, player2, token, _admin) =
        setup_with_dispute_period(100);
    let client = EscrowContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    let match_id = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "dp1616c1"),
        &Platform::Lichess,
    );
    client.deposit(&match_id, &player1);
    client.deposit(&match_id, &player2);

    env.ledger().set_sequence_number(1000);
    client.submit_result(&match_id, &Winner::Draw, &oracle);

    env.ledger().set_sequence_number(1100);
    client.finalize_match(&match_id);

    // Both players receive their stake back.
    assert_eq!(token_client.balance(&player1), 1000);
    assert_eq!(token_client.balance(&player2), 1000);
    assert_eq!(token_client.balance(&contract_id), 0);

    // Neither player should be able to claim again.
    let result_p1 = client.try_claim_vested_payout(&match_id, &player1);
    let result_p2 = client.try_claim_vested_payout(&match_id, &player2);
    assert!(
        result_p1.is_err(),
        "player1 claim_vested_payout must fail after finalize_match draw"
    );
    assert!(
        result_p2.is_err(),
        "player2 claim_vested_payout must fail after finalize_match draw"
    );

    // Balances unchanged.
    assert_eq!(token_client.balance(&player1), 1000);
    assert_eq!(token_client.balance(&player2), 1000);
    assert_eq!(token_client.balance(&contract_id), 0);
}

// ── resolve_dispute_by_vote ───────────────────────────────────────────────────

/// After `resolve_dispute_by_vote` (upheld result → player1 wins), the winner
/// must not be able to claim a second payout via `claim_vested_payout`.
/// Token balances must reflect exactly one payout.
#[test]
fn test_resolve_dispute_by_vote_no_double_payout_upheld() {
    let (env, contract_id, oracle, player1, player2, token, _admin) =
        setup_with_dispute_period(200);
    let client = EscrowContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    let match_id = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "dp1616d1"),
        &Platform::Lichess,
    );
    client.deposit(&match_id, &player1);
    client.deposit(&match_id, &player2);

    env.ledger().set_sequence_number(1000);
    client.submit_result(&match_id, &Winner::Player1, &oracle);

    // player2 opens a dispute.
    let dispute_id = client.dispute_oracle_result(
        &match_id,
        &player2,
        &String::from_str(
            &env,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ),
    );

    // player1 votes to uphold (no = false); player2 votes to overturn (yes = true).
    // yes=stake(player2)=900, no=stake(player1)=900 → tie → upheld (no majority overturn).
    client.vote_on_dispute(&dispute_id, &player1, &false);
    client.vote_on_dispute(&dispute_id, &player2, &true);

    // Advance past the voting deadline (1000 + VOTING_PERIOD_LEDGERS).
    env.ledger()
        .set_sequence_number(1000 + VOTING_PERIOD_LEDGERS);
    client.resolve_dispute_by_vote(&dispute_id);

    // player1 gets the pot (original oracle result upheld).
    // player2 loses their dispute bond (1 token at default rate).
    assert_eq!(
        token_client.balance(&player1), 1100,
        "player1 should have received the pot: 900 + 200 = 1100"
    );
    assert_eq!(
        token_client.balance(&contract_id), 0,
        "contract should hold 0 tokens after dispute resolution"
    );

    // A follow-up claim_vested_payout by player1 must fail.
    let result = client.try_claim_vested_payout(&match_id, &player1);
    assert!(
        result.is_err(),
        "claim_vested_payout must fail after resolve_dispute_by_vote has already paid out"
    );

    // Balances must be unchanged — exactly one payout.
    assert_eq!(
        token_client.balance(&player1), 1100,
        "player1 balance must not change after a rejected double-payout attempt"
    );
    assert_eq!(
        token_client.balance(&contract_id), 0,
        "contract balance must remain 0 after a rejected double-payout attempt"
    );
}

/// After `resolve_dispute_by_vote` with an overturned result (draw), neither
/// player should be able to claim again via `claim_vested_payout`.
#[test]
fn test_resolve_dispute_by_vote_no_double_payout_overturned() {
    let (env, contract_id, oracle, player1, player2, token, admin) =
        setup_with_dispute_period(200);
    let client = EscrowContractClient::new(&env, &contract_id);
    let token_client = TokenClient::new(&env, &token);

    let match_id = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "dp1616e1"),
        &Platform::Lichess,
    );
    client.deposit(&match_id, &player1);
    client.deposit(&match_id, &player2);

    env.ledger().set_sequence_number(1000);
    client.submit_result(&match_id, &Winner::Player1, &oracle);

    // player2 disputes.
    let dispute_id = client.dispute_oracle_result(
        &match_id,
        &player2,
        &String::from_str(
            &env,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ),
    );

    // player2 votes to overturn (yes = true) — majority overturn.
    client.vote_on_dispute(&dispute_id, &player2, &true);

    // Advance past the voting deadline.
    env.ledger()
        .set_sequence_number(1000 + VOTING_PERIOD_LEDGERS);
    client.resolve_dispute_by_vote(&dispute_id);

    // Result overturned → draw: each player gets their stake back.
    // The dispute bond is also refunded to player2 on a successful overturn.
    assert_eq!(
        token_client.balance(&player1), 1000,
        "player1 should get stake back on draw (overturned dispute)"
    );
    assert_eq!(
        token_client.balance(&player2), 1000,
        "player2 should get stake back + bond refunded on overturned dispute"
    );
    assert_eq!(token_client.balance(&contract_id), 0);

    // Neither player should be able to double-claim via claim_vested_payout.
    let result_p1 = client.try_claim_vested_payout(&match_id, &player1);
    let result_p2 = client.try_claim_vested_payout(&match_id, &player2);
    assert!(
        result_p1.is_err(),
        "player1 claim_vested_payout must fail after dispute overturn payout"
    );
    assert!(
        result_p2.is_err(),
        "player2 claim_vested_payout must fail after dispute overturn payout"
    );

    // Balances must be unchanged.
    assert_eq!(token_client.balance(&player1), 1000);
    assert_eq!(token_client.balance(&player2), 1000);
    assert_eq!(token_client.balance(&contract_id), 0);

    // Suppress unused variable warning — admin is needed by setup_with_dispute_period
    let _ = admin;
}
