//! End-to-end tests: deploy the release WASM contracts and drive the complete
//! match lifecycle — create match → deposit ×2 → oracle records result →
//! escrow settles → players claim the vested payout — verifying state and
//! balances at every step.

use e2e_tests::{
    deploy_and_fund, is_valid_chess_com_game_id, ChessComOutcome, ChessComPoller,
    MockChessComServer, World, MINT_AMOUNT, STAKE,
};
use escrow::{
    errors::Error as EscrowError,
    types::{MatchState, Platform as EscrowPlatform, Winner as EscrowWinner},
};
use oracle::{
    errors::Error as OracleError,
    types::{Platform as OraclePlatform, Winner as OracleWinner},
};
use soroban_sdk::{testutils::Address as _, Address, String};

/// Realistic Lichess game id (exactly 8 alphanumeric chars).
const GAME_ID: &str = "abcd1234";

/// Realistic Chess.com game id (7–12 ASCII digits).
const CHESS_COM_GAME_ID: &str = "96913734";

/// How long the oracle took to fetch + verify the result, in ms.
const RESPONSE_TIME_MS: u64 = 250;

/// Confidence score the oracle attaches to the result (0-100).
const CONFIDENCE: Option<u8> = Some(95);

/// Drives the full lifecycle for a given outcome and asserts every invariant
/// along the way:
///
/// 1. deploy (done in `deploy_and_fund`) — release WASM, not in-crate code
/// 2. create match → `Pending`
/// 3. deposit ×2 → `Active`, escrow holds `2 × stake`
/// 4. oracle records the verified result
/// 5. escrow settles → `Completed`, payout vests
/// 6. players claim → winner takes the pot, contract retains nothing
fn assert_full_lifecycle(winner: EscrowWinner, oracle_result: OracleWinner) {
    let World {
        env,
        escrow,
        oracle,
        token,
        token_client,
        oracle_admin,
        player1,
        player2,
        ..
    } = deploy_and_fund();

    // ── 1. Create match ──────────────────────────────────────────────────
    let id = escrow.create_match(
        &player1,
        &player2,
        &STAKE,
        &token,
        &String::from_str(&env, GAME_ID),
        &EscrowPlatform::Lichess,
    );
    assert_eq!(id, 0, "first match should be assigned id 0");

    let pending = escrow.get_match(&id);
    assert_eq!(pending.state, MatchState::Pending, "match starts Pending");
    assert_eq!(escrow.get_escrow_balance(&id), 0, "nothing in escrow yet");

    // ── 2. Both players deposit ──────────────────────────────────────────
    escrow.deposit(&id, &player1);
    assert!(
        !escrow.is_funded(&id),
        "not funded after only player1 deposits"
    );
    assert_eq!(escrow.get_escrow_balance(&id), STAKE);

    escrow.deposit(&id, &player2);
    assert!(escrow.is_funded(&id), "funded once both players deposit");
    assert_eq!(escrow.get_escrow_balance(&id), 2 * STAKE);
    assert_eq!(escrow.get_match(&id).state, MatchState::Active);

    // The escrow contract holds 2× stake; players are down their stakes.
    assert_eq!(token_client.balance(&escrow.address), 2 * STAKE);
    assert_eq!(token_client.balance(&player1), MINT_AMOUNT - STAKE);
    assert_eq!(token_client.balance(&player2), MINT_AMOUNT - STAKE);

    // ── 3. Oracle records the verified result ────────────────────────────
    oracle.submit_result(
        &id,
        &String::from_str(&env, GAME_ID),
        &OraclePlatform::Lichess,
        &oracle_result,
        &RESPONSE_TIME_MS,
        &CONFIDENCE,
    );
    assert!(oracle.has_result(&id), "oracle stores the result");
    let entry = oracle.get_result(&id);
    assert_eq!(entry.game_id, String::from_str(&env, GAME_ID));
    assert_eq!(entry.result, oracle_result);
    assert_eq!(entry.confidence, CONFIDENCE);

    // ── 4. Oracle settles the match in escrow → payout vests ─────────────
    escrow.submit_result(&id, &winner, &oracle_admin, &CONFIDENCE);
    let settled = escrow.get_match(&id);
    assert_eq!(
        settled.state,
        MatchState::Completed,
        "match completes on settle"
    );
    assert_eq!(settled.winner, winner);
    assert_eq!(escrow.get_escrow_balance(&id), 0);

    // ── 5. Players claim the vested payout ───────────────────────────────
    match winner {
        EscrowWinner::Player1 => {
            escrow.claim_vested_payout(&id, &player1);
            // The loser cannot claim anything — the pot goes to the winner.
            assert_eq!(
                escrow.try_claim_vested_payout(&id, &player2),
                Err(Ok(EscrowError::Unauthorized)),
                "loser must not be able to claim a payout"
            );
            assert_eq!(token_client.balance(&player1), MINT_AMOUNT + STAKE);
            assert_eq!(token_client.balance(&player2), MINT_AMOUNT - STAKE);
        }
        EscrowWinner::Player2 => {
            escrow.claim_vested_payout(&id, &player2);
            assert_eq!(
                escrow.try_claim_vested_payout(&id, &player1),
                Err(Ok(EscrowError::Unauthorized)),
                "loser must not be able to claim a payout"
            );
            assert_eq!(token_client.balance(&player1), MINT_AMOUNT - STAKE);
            assert_eq!(token_client.balance(&player2), MINT_AMOUNT + STAKE);
        }
        EscrowWinner::Draw => {
            escrow.claim_vested_payout(&id, &player1);
            escrow.claim_vested_payout(&id, &player2);
            assert_eq!(token_client.balance(&player1), MINT_AMOUNT);
            assert_eq!(token_client.balance(&player2), MINT_AMOUNT);
        }
        EscrowWinner::None => unreachable!("tests only settle real outcomes"),
    }

    assert_eq!(
        token_client.balance(&escrow.address),
        0,
        "contract must not retain funds after payout"
    );
}

#[test]
fn test_full_lifecycle_player1_wins() {
    assert_full_lifecycle(EscrowWinner::Player1, OracleWinner::Player1);
}

#[test]
fn test_full_lifecycle_player2_wins() {
    assert_full_lifecycle(EscrowWinner::Player2, OracleWinner::Player2);
}

#[test]
fn test_full_lifecycle_draw() {
    assert_full_lifecycle(EscrowWinner::Draw, OracleWinner::Draw);
}

/// An impostor account must not be able to settle a match: the match stays
/// Active and the funds stay in escrow.
#[test]
fn test_non_oracle_cannot_submit_result() {
    let World {
        env,
        escrow,
        token,
        token_client,
        player1,
        player2,
        ..
    } = deploy_and_fund();

    let id = escrow.create_match(
        &player1,
        &player2,
        &STAKE,
        &token,
        &String::from_str(&env, "efgh5678"),
        &EscrowPlatform::Lichess,
    );
    escrow.deposit(&id, &player1);
    escrow.deposit(&id, &player2);

    let impostor = Address::generate(&env);
    let result = escrow.try_submit_result(&id, &EscrowWinner::Player1, &impostor, &None);
    assert_eq!(
        result,
        Err(Ok(EscrowError::Unauthorized)),
        "non-oracle submit_result must be rejected"
    );

    // Match untouched — still Active with both stakes still in escrow.
    assert_eq!(escrow.get_match(&id).state, MatchState::Active);
    assert_eq!(token_client.balance(&escrow.address), 2 * STAKE);
}

/// The oracle contract must refuse a second result submission for the same
/// match — no overwriting a recorded result.
#[test]
fn test_oracle_duplicate_submit_rejected() {
    let World {
        env,
        escrow,
        oracle,
        token,
        player1,
        player2,
        ..
    } = deploy_and_fund();

    let id = escrow.create_match(
        &player1,
        &player2,
        &STAKE,
        &token,
        &String::from_str(&env, "ijkl9012"),
        &EscrowPlatform::Lichess,
    );

    oracle.submit_result(
        &id,
        &String::from_str(&env, "ijkl9012"),
        &OraclePlatform::Lichess,
        &OracleWinner::Player1,
        &RESPONSE_TIME_MS,
        &CONFIDENCE,
    );
    assert!(oracle.has_result(&id), "first submission is recorded");

    let result = oracle.try_submit_result(
        &id,
        &String::from_str(&env, "ijkl9012"),
        &OraclePlatform::Lichess,
        &OracleWinner::Player2,
        &RESPONSE_TIME_MS,
        &CONFIDENCE,
    );
    assert_eq!(
        result,
        Err(Ok(OracleError::AlreadySubmitted)),
        "a second submission for the same match must be rejected"
    );

    // The originally recorded result is untouched.
    assert_eq!(oracle.get_result(&id).result, OracleWinner::Player1);
}

/// A player cannot claim a payout before the oracle settles the match.
#[test]
fn test_claim_before_settlement_rejected() {
    let World {
        env,
        escrow,
        token,
        player1,
        player2,
        ..
    } = deploy_and_fund();

    let id = escrow.create_match(
        &player1,
        &player2,
        &STAKE,
        &token,
        &String::from_str(&env, "mnop3456"),
        &EscrowPlatform::Lichess,
    );
    escrow.deposit(&id, &player1);
    escrow.deposit(&id, &player2);
    assert_eq!(escrow.get_match(&id).state, MatchState::Active);

    let result = escrow.try_claim_vested_payout(&id, &player1);
    assert_eq!(
        result,
        Err(Ok(EscrowError::InvalidState)),
        "claiming before settlement must be rejected"
    );
}

/// The oracle cannot settle a match that was never fully funded.
#[test]
fn test_submit_result_on_unfunded_match_rejected() {
    let World {
        env,
        escrow,
        oracle_admin,
        token,
        token_client,
        player1,
        player2,
        ..
    } = deploy_and_fund();

    let id = escrow.create_match(
        &player1,
        &player2,
        &STAKE,
        &token,
        &String::from_str(&env, "qrst7890"),
        &EscrowPlatform::Lichess,
    );
    // Only player1 deposits — match stays Pending and unfunded.
    escrow.deposit(&id, &player1);

    let result = escrow.try_submit_result(&id, &EscrowWinner::Player1, &oracle_admin, &CONFIDENCE);
    assert_eq!(
        result,
        Err(Ok(EscrowError::NotFunded)),
        "settling an unfunded match must be rejected"
    );

    assert_eq!(escrow.get_match(&id).state, MatchState::Pending);
    assert_eq!(token_client.balance(&escrow.address), STAKE);
}

// ── Chess.com E2E Poller Lifecycle Tests ─────────────────────────────────────

/// Drives the full Chess.com lifecycle through the mock Chess.com API server
/// and oracle poller:
///
/// 1. Start [`MockChessComServer`] and register the game as `InProgress`
/// 2. Create match (`Platform::ChessDotCom`) → `Pending`
/// 3. Deposit ×2 → `Active`, escrow holds `2 × stake`
/// 4. Poll while game is still in progress → match stays `Active`
/// 5. Mock Chess.com game finishes (`WhiteWin`, `BlackWin`, or `Draw`)
/// 6. Oracle poller fetches result from mock server and submits to `oracle` + `escrow`
/// 7. Players claim vested payout → assert final balances and contract state
fn assert_chess_com_poller_lifecycle(outcome: ChessComOutcome) {
    let world = deploy_and_fund();
    let server = MockChessComServer::start();
    let poller = ChessComPoller::new(&server, CONFIDENCE);

    // Register the game on the mock Chess.com server as still in progress.
    server.register_in_progress(CHESS_COM_GAME_ID);

    // ── 1. Create Chess.com match ────────────────────────────────────────
    let id = world.escrow.create_match(
        &world.player1,
        &world.player2,
        &STAKE,
        &world.token,
        &String::from_str(&world.env, CHESS_COM_GAME_ID),
        &EscrowPlatform::ChessDotCom,
    );
    assert_eq!(id, 0, "first match should be assigned id 0");

    let pending = world.escrow.get_match(&id);
    assert_eq!(pending.state, MatchState::Pending, "match starts Pending");
    assert_eq!(pending.platform, EscrowPlatform::ChessDotCom);
    assert_eq!(
        world.escrow.get_escrow_balance(&id),
        0,
        "nothing in escrow yet"
    );

    // ── 2. Both players deposit ──────────────────────────────────────────
    world.escrow.deposit(&id, &world.player1);
    assert!(
        !world.escrow.is_funded(&id),
        "not funded after only player1 deposits"
    );
    assert_eq!(world.escrow.get_escrow_balance(&id), STAKE);

    world.escrow.deposit(&id, &world.player2);
    assert!(
        world.escrow.is_funded(&id),
        "funded once both players deposit"
    );
    assert_eq!(world.escrow.get_escrow_balance(&id), 2 * STAKE);
    assert_eq!(world.escrow.get_match(&id).state, MatchState::Active);

    assert_eq!(world.token_client.balance(&world.escrow.address), 2 * STAKE);
    assert_eq!(
        world.token_client.balance(&world.player1),
        MINT_AMOUNT - STAKE
    );
    assert_eq!(
        world.token_client.balance(&world.player2),
        MINT_AMOUNT - STAKE
    );

    // ── 3. Oracle polls while Chess.com game is still in progress ────────
    let settled_while_in_progress = poller
        .tick(&world)
        .expect("polling in-progress Chess.com game must not error");
    assert!(
        settled_while_in_progress.is_empty(),
        "in-progress Chess.com game must not be settled prematurely"
    );
    assert_eq!(
        server.request_count(CHESS_COM_GAME_ID),
        1,
        "poller should have queried the mock Chess.com server once"
    );
    assert!(
        !world.oracle.has_result(&id),
        "no oracle result while in progress"
    );
    assert_eq!(world.escrow.get_match(&id).state, MatchState::Active);

    // ── 4. Mock Chess.com game finishes → Oracle poller submits result ───
    server.finish_game(CHESS_COM_GAME_ID, outcome);

    let settled_ids = poller
        .tick(&world)
        .expect("polling finished Chess.com game must succeed");
    assert_eq!(
        settled_ids,
        vec![id],
        "poller must settle the active Chess.com match"
    );
    assert_eq!(
        server.request_count(CHESS_COM_GAME_ID),
        2,
        "poller should have queried the mock Chess.com server a second time"
    );

    // Verify oracle contract recorded the Chess.com result.
    let expected_oracle_winner = outcome.to_oracle_winner();
    assert!(
        world.oracle.has_result(&id),
        "oracle stores the Chess.com result"
    );
    let entry = world.oracle.get_result(&id);
    assert_eq!(
        entry.game_id,
        String::from_str(&world.env, CHESS_COM_GAME_ID)
    );
    assert_eq!(entry.platform, OraclePlatform::ChessDotCom);
    assert_eq!(entry.result, expected_oracle_winner);
    assert_eq!(entry.confidence, CONFIDENCE);
    assert!(entry.response_time_ms >= 1);

    // Verify escrow contract settled the match.
    let expected_escrow_winner = outcome.to_escrow_winner();
    let settled = world.escrow.get_match(&id);
    assert_eq!(
        settled.state,
        MatchState::Completed,
        "match completes on settle"
    );
    assert_eq!(settled.winner, expected_escrow_winner);
    assert_eq!(world.escrow.get_escrow_balance(&id), 0);

    // ── 5. Players claim vested payout & assert balances ─────────────────
    match expected_escrow_winner {
        EscrowWinner::Player1 => {
            world.escrow.claim_vested_payout(&id, &world.player1);
            assert_eq!(
                world.escrow.try_claim_vested_payout(&id, &world.player2),
                Err(Ok(EscrowError::Unauthorized)),
                "loser must not be able to claim a payout"
            );
            assert_eq!(
                world.token_client.balance(&world.player1),
                MINT_AMOUNT + STAKE
            );
            assert_eq!(
                world.token_client.balance(&world.player2),
                MINT_AMOUNT - STAKE
            );
        }
        EscrowWinner::Player2 => {
            world.escrow.claim_vested_payout(&id, &world.player2);
            assert_eq!(
                world.escrow.try_claim_vested_payout(&id, &world.player1),
                Err(Ok(EscrowError::Unauthorized)),
                "loser must not be able to claim a payout"
            );
            assert_eq!(
                world.token_client.balance(&world.player1),
                MINT_AMOUNT - STAKE
            );
            assert_eq!(
                world.token_client.balance(&world.player2),
                MINT_AMOUNT + STAKE
            );
        }
        EscrowWinner::Draw => {
            world.escrow.claim_vested_payout(&id, &world.player1);
            world.escrow.claim_vested_payout(&id, &world.player2);
            assert_eq!(world.token_client.balance(&world.player1), MINT_AMOUNT);
            assert_eq!(world.token_client.balance(&world.player2), MINT_AMOUNT);
        }
        EscrowWinner::None => unreachable!("tests only settle real outcomes"),
    }

    assert_eq!(
        world.token_client.balance(&world.escrow.address),
        0,
        "contract must not retain funds after Chess.com payout"
    );
}

#[test]
fn test_chess_com_poller_lifecycle_player1_wins() {
    assert_chess_com_poller_lifecycle(ChessComOutcome::WhiteWin);
}

#[test]
fn test_chess_com_poller_lifecycle_player2_wins() {
    assert_chess_com_poller_lifecycle(ChessComOutcome::BlackWin);
}

#[test]
fn test_chess_com_poller_lifecycle_draw() {
    assert_chess_com_poller_lifecycle(ChessComOutcome::Draw);
}

#[test]
fn test_chess_com_mock_server_and_game_id_validation() {
    assert!(is_valid_chess_com_game_id("96913734"));
    assert!(is_valid_chess_com_game_id("123456789012"));
    assert!(!is_valid_chess_com_game_id(""));
    assert!(!is_valid_chess_com_game_id("123456")); // fewer than 7 digits
    assert!(!is_valid_chess_com_game_id("abcd1234")); // non-numeric Lichess ID

    let server = MockChessComServer::start();

    // Unknown game returns None (404).
    let (unknown, _) = server.fetch_game_result("11111111").unwrap();
    assert_eq!(unknown, None);

    // In-progress game returns None until finished.
    server.register_in_progress("22222222");
    let (in_progress, _) = server.fetch_game_result("22222222").unwrap();
    assert_eq!(in_progress, None);

    // Finishing the game transitions the endpoint response to the final outcome.
    server.finish_game("22222222", ChessComOutcome::Draw);
    let (finished_draw, _) = server.fetch_game_result("22222222").unwrap();
    assert_eq!(finished_draw, Some(ChessComOutcome::Draw));
    assert_eq!(server.request_count("22222222"), 2);
}
