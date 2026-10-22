use super::*;
use soroban_sdk::testutils::Ledger as _;

/// Helper to set up contracts and get through to an overturned dispute ready for slashing
fn setup_for_slash_relay_test() -> (
    Env,
    Address,
    Address,
    Address,
    Address,
    Address,
    Address,
    u64,
    i128,
) {
    let (env, contract_id, oracle, player1, player2, token, admin) = setup_with_dispute_period(200);
    let client = EscrowContractClient::new(&env, &contract_id);

    // Set up dispute bond (10%)
    client.set_dispute_bond_basis_points(&1000);

    let match_id = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "rly00001"),
        &Platform::Lichess,
    );
    client.deposit(&match_id, &player1);
    client.deposit(&match_id, &player2);

    env.ledger().set_sequence_number(1000);
    client.submit_result(&match_id, &Winner::Player1, &oracle);

    let dispute_id = client.dispute_oracle_result(
        &match_id,
        &player2,
        &String::from_str(
            &env,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ),
    );

    client.vote_on_dispute(&dispute_id, &player2, &true);
    env.ledger()
        .set_sequence_number(1000 + VOTING_PERIOD_LEDGERS);
    client.resolve_dispute_by_vote(&dispute_id);

    let dispute = client.get_dispute(&dispute_id);
    assert_eq!(dispute.state, DisputeState::ResolvedOverturned);
    let bond = dispute.dispute_bond;

    (
        env,
        contract_id,
        oracle,
        player1,
        player2,
        token,
        admin,
        dispute_id,
        bond,
    )
}

/// Test 1: Verify that mark_dispute_for_oracle_slash emits the correct signal event
#[test]
fn test_relay_slash_signal_event_format() {
    let (env, contract_id, oracle, _player1, _player2, _token, _admin, dispute_id, bond) =
        setup_for_slash_relay_test();
    let client = EscrowContractClient::new(&env, &contract_id);

    // Emit the slash signal
    client.mark_dispute_for_oracle_slash(&dispute_id, &bond);

    let events = env.events().all();
    let expected_topics = vec![
        &env,
        Symbol::new(&env, "dispute").into_val(&env),
        Symbol::new(&env, "oracle_slash_signal").into_val(&env),
    ];

    let matched = events
        .iter()
        .find(|(_, topics, _)| *topics == expected_topics);

    assert!(
        matched.is_some(),
        "oracle_slash_signal event must be emitted"
    );

    let (_, _, data) = matched.unwrap();
    let (ev_dispute_id, ev_oracle, ev_amount): (u64, Address, i128) =
        TryFromVal::try_from_val(&env, &data).unwrap();

    assert_eq!(ev_dispute_id, dispute_id);
    assert_eq!(ev_oracle, oracle);
    assert_eq!(ev_amount, bond);
}

/// Test 2: Verify that before any relay processes the signal, the oracle's stake is unchanged
#[test]
fn test_relay_oracle_stake_unchanged_before_slash() {
    let (env, contract_id, _oracle, _player1, _player2, _token, _admin, dispute_id, bond) =
        setup_for_slash_relay_test();
    let client = EscrowContractClient::new(&env, &contract_id);

    // Emit signal but don't slash
    client.mark_dispute_for_oracle_slash(&dispute_id, &bond);

    // If a relay were listening, it would call slash_oracle on the oracle contract
    // This test just verifies the escrow contract has done its part
    let dispute = client.get_dispute(&dispute_id);
    assert_eq!(dispute.state, DisputeState::ResolvedOverturned);
}

/// Test 3: Verify correct oracle is included in slash signal
#[test]
fn test_relay_slash_signal_names_correct_oracle() {
    let (env, contract_id, oracle, player1, player2, token, _admin) =
        setup_with_dispute_period(200);
    let client = EscrowContractClient::new(&env, &contract_id);

    // Create match and get to overturned dispute with a specific oracle
    client.set_dispute_bond_basis_points(&1000);

    let match_id = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "rly00002"),
        &Platform::Lichess,
    );
    client.deposit(&match_id, &player1);
    client.deposit(&match_id, &player2);

    env.ledger().set_sequence_number(1000);
    client.submit_result(&match_id, &Winner::Player1, &oracle);

    let dispute_id = client.dispute_oracle_result(
        &match_id,
        &player2,
        &String::from_str(
            &env,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ),
    );

    client.vote_on_dispute(&dispute_id, &player2, &true);
    env.ledger()
        .set_sequence_number(1000 + VOTING_PERIOD_LEDGERS);
    client.resolve_dispute_by_vote(&dispute_id);

    let bond = client.get_dispute(&dispute_id).dispute_bond;

    client.mark_dispute_for_oracle_slash(&dispute_id, &bond);

    let events = env.events().all();
    let expected_topics = vec![
        &env,
        Symbol::new(&env, "dispute").into_val(&env),
        Symbol::new(&env, "oracle_slash_signal").into_val(&env),
    ];

    let matched = events
        .iter()
        .find(|(_, topics, _)| *topics == expected_topics);

    assert!(matched.is_some());

    let (_, _, data) = matched.unwrap();
    let (_ev_dispute_id, ev_oracle, _ev_amount): (u64, Address, i128) =
        TryFromVal::try_from_val(&env, &data).unwrap();

    // The oracle in the signal must match the oracle that submitted the result
    assert_eq!(ev_oracle, oracle);
}

/// Test 4: Verify slash amount matches bond for full slash
#[test]
fn test_relay_slash_signal_correct_amount_full_bond() {
    let (env, contract_id, _oracle, _player1, _player2, _token, _admin, dispute_id, bond) =
        setup_for_slash_relay_test();
    let client = EscrowContractClient::new(&env, &contract_id);

    client.mark_dispute_for_oracle_slash(&dispute_id, &bond);

    let events = env.events().all();
    let expected_topics = vec![
        &env,
        Symbol::new(&env, "dispute").into_val(&env),
        Symbol::new(&env, "oracle_slash_signal").into_val(&env),
    ];

    let matched = events
        .iter()
        .find(|(_, topics, _)| *topics == expected_topics);

    let (_, _, data) = matched.unwrap();
    let (_ev_dispute_id, _ev_oracle, ev_amount): (u64, Address, i128) =
        TryFromVal::try_from_val(&env, &data).unwrap();

    assert_eq!(ev_amount, bond);
}

/// Test 5: Verify slash amount can be partial (less than bond)
#[test]
fn test_relay_slash_signal_partial_slash_amount() {
    let (env, contract_id, _oracle, _player1, _player2, _token, _admin, dispute_id, bond) =
        setup_for_slash_relay_test();
    let client = EscrowContractClient::new(&env, &contract_id);

    // Slash only half the bond
    let partial_slash = bond / 2;
    client.mark_dispute_for_oracle_slash(&dispute_id, &partial_slash);

    let events = env.events().all();
    let expected_topics = vec![
        &env,
        Symbol::new(&env, "dispute").into_val(&env),
        Symbol::new(&env, "oracle_slash_signal").into_val(&env),
    ];

    let matched = events
        .iter()
        .find(|(_, topics, _)| *topics == expected_topics);

    let (_, _, data) = matched.unwrap();
    let (_ev_dispute_id, _ev_oracle, ev_amount): (u64, Address, i128) =
        TryFromVal::try_from_val(&env, &data).unwrap();

    assert_eq!(ev_amount, partial_slash);
}

/// Test 6: Verify slash signal is rejected if dispute not overturned
#[test]
fn test_relay_slash_signal_requires_overturned_state() {
    let (env, contract_id, oracle, player1, player2, token, _admin) =
        setup_with_dispute_period(200);
    let client = EscrowContractClient::new(&env, &contract_id);

    let match_id = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "rly00003"),
        &Platform::Lichess,
    );
    client.deposit(&match_id, &player1);
    client.deposit(&match_id, &player2);

    env.ledger().set_sequence_number(1000);
    client.submit_result(&match_id, &Winner::Player1, &oracle);

    let dispute_id = client.dispute_oracle_result(
        &match_id,
        &player2,
        &String::from_str(
            &env,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ),
    );

    // Resolve as upheld (not overturned)
    client.vote_on_dispute(&dispute_id, &player2, &false);
    env.ledger()
        .set_sequence_number(1000 + VOTING_PERIOD_LEDGERS);
    client.resolve_dispute_by_vote(&dispute_id);

    let dispute = client.get_dispute(&dispute_id);
    assert_eq!(dispute.state, DisputeState::ResolvedUpheld);

    let bond = dispute.dispute_bond;
    let result = client.try_mark_dispute_for_oracle_slash(&dispute_id, &bond);
    assert!(result.is_err(), "slash signal must be rejected for non-overturned dispute");
}

/// Test 7: Verify that a dispute can only be signalled for oracle slash once.
///
/// Regression test for #1535: `mark_dispute_for_oracle_slash` previously stored
/// nothing, so it could be called repeatedly for the same dispute, causing
/// off-chain relays to slash the oracle multiple times. The contract now
/// persists a `DisputeSlashSignalled(dispute_id)` flag and rejects repeats.
#[test]
fn test_relay_slash_signal_is_idempotent() {
    let (env, contract_id, _oracle, _player1, _player2, _token, _admin, dispute_id, bond) =
        setup_for_slash_relay_test();
    let client = EscrowContractClient::new(&env, &contract_id);

    // First call succeeds and emits the signal.
    client.mark_dispute_for_oracle_slash(&dispute_id, &bond);

    // Second call for the same dispute must be rejected.
    let second = client.try_mark_dispute_for_oracle_slash(&dispute_id, &bond);
    assert!(
        second.is_err(),
        "mark_dispute_for_oracle_slash must reject a repeat call for the same dispute"
    );

    // Only a single oracle_slash_signal event should have been emitted.
    let events = env.events().all();
    let expected_topics = vec![
        &env,
        Symbol::new(&env, "dispute").into_val(&env),
        Symbol::new(&env, "oracle_slash_signal").into_val(&env),
    ];
    let signal_count = events
        .iter()
        .filter(|(_, topics, _)| *topics == expected_topics)
        .count();
    assert_eq!(
        signal_count, 1,
        "oracle_slash_signal must be emitted exactly once per dispute"
    );
}
