use super::*;

// #1166 — submit_result_batch settles multiple matches in one call

#[test]
fn test_submit_result_batch_all_success() {
    let (env, contract_id, _oracle, player1, player2, token, _admin) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let match_a = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "885b6556"),
        &Platform::Lichess,
    );
    let match_b = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "8372756a"),
        &Platform::Lichess,
    );

    client.deposit(&match_a, &player1);
    client.deposit(&match_a, &player2);
    client.deposit(&match_b, &player1);
    client.deposit(&match_b, &player2);

    let oracle = client.get_oracle();
    let batch = soroban_sdk::vec![&env, (match_a, Winner::Player1), (match_b, Winner::Player2),];

    let outcomes = client.submit_result_batch(&batch, &oracle);

    assert_eq!(outcomes.len(), 2);
    assert_eq!(outcomes.get(0).unwrap(), None);
    assert_eq!(outcomes.get(1).unwrap(), None);

    assert_eq!(client.get_match(&match_a).state, MatchState::Completed);
    assert_eq!(client.get_match(&match_a).winner, Winner::Player1);
    assert_eq!(client.get_match(&match_b).state, MatchState::Completed);
    assert_eq!(client.get_match(&match_b).winner, Winner::Player2);
}

#[test]
fn test_submit_result_batch_partial_failure() {
    let (env, contract_id, _oracle, player1, player2, token, _admin) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // match_a is fully funded and will succeed.
    let match_a = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "7e081169"),
        &Platform::Lichess,
    );
    client.deposit(&match_a, &player1);
    client.deposit(&match_a, &player2);

    // match_b only has one deposit, so it's still Pending — submit_result
    // fails with NotFunded (not yet fully funded).
    let match_b = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "8b4685e8"),
        &Platform::Lichess,
    );
    client.deposit(&match_b, &player1);

    // match_c does not exist at all, so submit_result will fail with MatchNotFound.
    let match_c = client.get_match_count() + 999;

    let oracle = client.get_oracle();
    let batch = soroban_sdk::vec![
        &env,
        (match_a, Winner::Player1),
        (match_b, Winner::Player2),
        (match_c, Winner::Draw),
    ];

    let outcomes = client.submit_result_batch(&batch, &oracle);

    assert_eq!(outcomes.len(), 3);
    assert_eq!(outcomes.get(0).unwrap(), None);
    assert_eq!(outcomes.get(1).unwrap(), Some(Error::NotFunded));
    assert_eq!(outcomes.get(2).unwrap(), Some(Error::MatchNotFound));

    // The successful match settled, and the failing matches were left untouched.
    assert_eq!(client.get_match(&match_a).state, MatchState::Completed);
    assert_eq!(client.get_match(&match_b).state, MatchState::Pending);
}

// #1308 — NotFunded must be reported per-entry for every unfunded match in a
// batch, not just skipped or mislabeled as InvalidState.
#[test]
fn test_submit_result_batch_mixed_funded_and_unfunded_reports_not_funded() {
    let (env, contract_id, _oracle, player1, player2, token, _admin) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // Fully funded — should settle successfully.
    let funded = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "batch_funded1"),
        &Platform::Lichess,
    );
    client.deposit(&funded, &player1);
    client.deposit(&funded, &player2);

    // No deposits at all — still Pending.
    let unfunded_none = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "batch_unfunded_none"),
        &Platform::Lichess,
    );

    // Only one side deposited — still Pending.
    let unfunded_partial = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "batch_unfunded_partial"),
        &Platform::Lichess,
    );
    client.deposit(&unfunded_partial, &player1);

    let oracle = client.get_oracle();
    let batch = soroban_sdk::vec![
        &env,
        (funded, Winner::Player1),
        (unfunded_none, Winner::Player1),
        (unfunded_partial, Winner::Player2),
    ];

    let outcomes = client.submit_result_batch(&batch, &oracle);

    assert_eq!(outcomes.len(), 3);
    assert_eq!(outcomes.get(0).unwrap(), None);
    assert_eq!(outcomes.get(1).unwrap(), Some(Error::NotFunded));
    assert_eq!(outcomes.get(2).unwrap(), Some(Error::NotFunded));

    assert_eq!(client.get_match(&funded).state, MatchState::Completed);
    assert_eq!(client.get_match(&unfunded_none).state, MatchState::Pending);
    assert_eq!(
        client.get_match(&unfunded_partial).state,
        MatchState::Pending
    );
}

// #1744 — batch entries are processed independently: a failure in the middle
// entry must not block the valid entries before or after it.
#[test]
fn test_submit_result_batch_mixed_valid_invalid_valid() {
    let (env, contract_id, _oracle, player1, player2, token, _admin) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    // Entry 0 — fully funded, will settle successfully.
    let match_a = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "mixed_valid_0"),
        &Platform::Lichess,
    );
    client.deposit(&match_a, &player1);
    client.deposit(&match_a, &player2);

    // Entry 1 — only one side deposited, so it's still Pending and
    // submit_result fails with NotFunded.
    let match_b = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "mixed_invalid_1"),
        &Platform::Lichess,
    );
    client.deposit(&match_b, &player1);

    // Entry 2 — fully funded, will settle successfully.
    let match_c = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "mixed_valid_2"),
        &Platform::Lichess,
    );
    client.deposit(&match_c, &player1);
    client.deposit(&match_c, &player2);

    let oracle = client.get_oracle();
    let batch = soroban_sdk::vec![
        &env,
        (match_a, Winner::Player1),
        (match_b, Winner::Player2),
        (match_c, Winner::Draw),
    ];

    let outcomes = client.submit_result_batch(&batch, &oracle);

    assert_eq!(outcomes.len(), 3);
    assert_eq!(outcomes.get(0).unwrap(), None);
    assert_eq!(outcomes.get(1).unwrap(), Some(Error::NotFunded));
    assert_eq!(outcomes.get(2).unwrap(), None);

    // Valid entries settled despite the failure in the middle.
    assert_eq!(client.get_match(&match_a).state, MatchState::Completed);
    assert_eq!(client.get_match(&match_a).winner, Winner::Player1);
    assert_eq!(client.get_match(&match_c).state, MatchState::Completed);
    assert_eq!(client.get_match(&match_c).winner, Winner::Draw);

    // The invalid entry was left untouched.
    assert_eq!(client.get_match(&match_b).state, MatchState::Pending);
}

#[test]
fn test_submit_result_batch_already_confirmed_match() {
    let (env, contract_id, _oracle, player1, player2, token, _admin) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let match_a = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "a1b2c3d4"),
        &Platform::Lichess,
    );
    client.deposit(&match_a, &player1);
    client.deposit(&match_a, &player2);

    let oracle = client.get_oracle();

    // First submission settles the match.
    let first = client.submit_result(&match_a, &Winner::Player1, &oracle);
    assert_eq!(first, None);

    // Second submission via batch should report InvalidState for the
    // already-completed match.
    let batch = soroban_sdk::vec![&env, (match_a, Winner::Player2)];
    let outcomes = client.submit_result_batch(&batch, &oracle);

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes.get(0).unwrap(), Some(Error::InvalidState));
    assert_eq!(client.get_match(&match_a).winner, Winner::Player1);
}

// #1772 — submit_result_batch must thread the supplied game_id through to the
// stored oracle record instead of dropping it.
#[test]
fn test_submit_result_batch_preserves_game_id() {
    let (env, contract_id, _oracle, player1, player2, token, _admin) = setup();
    let client = EscrowContractClient::new(&env, &contract_id);

    let match_a = client.create_match(
        &player1,
        &player2,
        &100,
        &token,
        &String::from_str(&env, "batch_game_id_0"),
        &Platform::Lichess,
    );
    client.deposit(&match_a, &player1);
    client.deposit(&match_a, &player2);

    let oracle = client.get_oracle();
    let game_id = String::from_str(&env, "game-abc-123");
    let batch = soroban_sdk::vec![&env, (match_a, Winner::Player1, game_id.clone())];

    let outcomes = client.submit_result_batch(&batch, &oracle);

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes.get(0).unwrap(), None);
    assert_eq!(client.get_match(&match_a).state, MatchState::Completed);

    let record = client.get_oracle_record(&match_a).unwrap();
    assert_eq!(record.game_id, game_id);
}
