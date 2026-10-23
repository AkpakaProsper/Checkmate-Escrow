//! E2E test harness for the Checkmate Escrow + Oracle contracts.
//!
//! Unlike the unit tests in `contracts/*` (which register the contracts
//! natively, compiled into the test binary), these tests deploy the *release
//! WASM artifacts* — the exact bytecode that would be shipped to a live
//! network — into the sandboxed Soroban host and drive the complete match
//! lifecycle through the generated contract clients:
//!
//! create match → deposit ×2 → oracle records the result → escrow settles →
//! players claim the vested payout.

use escrow::EscrowContractClient;
use oracle::OracleContractClient;
use soroban_sdk::{
    testutils::Address as _,
    token::{Client as TokenClient, StellarAssetClient},
    Address, Env,
};

/// Stake used for every match in these tests.
pub const STAKE: i128 = 100;

/// Amount of the stake token minted to each player.
pub const MINT_AMOUNT: i128 = 1_000;

/// A fully deployed and initialized world: escrow + oracle contracts (both
/// loaded from release WASM), a Stellar asset contract as the stake token,
/// and two funded players.
///
/// The clients carry a (phantom) lifetime from soroban-sdk's generated client
/// types; since the harness never uses the per-client auth-mock builders, the
/// world is constructed with `'static`.
pub struct World<'a> {
    pub env: Env,
    pub escrow: EscrowContractClient<'a>,
    pub oracle: OracleContractClient<'a>,
    pub token: Address,
    pub token_client: TokenClient<'a>,
    /// The oracle service account: admin of the oracle contract and the
    /// address the escrow contract trusts to submit results.
    pub oracle_admin: Address,
    pub admin: Address,
    pub player1: Address,
    pub player2: Address,
}

/// Load a release WASM artifact built by `scripts/test.sh`.
///
/// The tests deliberately run against the compiled bytecode rather than the
/// in-crate Rust code so that what gets validated is what would be deployed.
pub fn load_wasm(name: &str) -> Vec<u8> {
    // wasm32v1-none is the Soroban target (core wasm 1.0 only). Building for
    // wasm32-unknown-unknown on recent Rust emits reference-types /
    // multi-value instructions that the Soroban host validator rejects at
    // deployment — the E2E suite would catch exactly this.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../target/wasm32v1-none/release/{name}.wasm"));
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "E2E tests need the release WASM artifact at {}. Build it first with \
             `cargo build --target wasm32v1-none --release -p escrow -p oracle` \
             (or `scripts/test.sh`). Error: {e}",
            path.display()
        )
    })
}

/// Deploy the escrow and oracle contracts from their release WASM artifacts,
/// deploy a Stellar asset contract, initialize everything, and fund the
/// players.
pub fn deploy_and_fund() -> World<'static> {
    let mut env = Env::default();
    // Don't write test_snapshots/*.json files to disk when the env is dropped.
    env.set_config(soroban_sdk::testutils::EnvTestConfig {
        capture_snapshot_at_drop: false,
    });
    env.mock_all_auths();

    // Deploy the actual release bytecode — this is what gets shipped.
    let escrow_wasm = load_wasm("escrow");
    let oracle_wasm = load_wasm("oracle");
    let escrow_id = env.register_contract_wasm(None, escrow_wasm.as_slice());
    let oracle_id = env.register_contract_wasm(None, oracle_wasm.as_slice());

    let oracle_admin = Address::generate(&env);
    let admin = Address::generate(&env);
    let player1 = Address::generate(&env);
    let player2 = Address::generate(&env);

    // USDC-style Stellar asset, minted to both players.
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token);
    let asset_client = StellarAssetClient::new(&env, &token);
    asset_client.mint(&player1, &MINT_AMOUNT);
    asset_client.mint(&player2, &MINT_AMOUNT);

    // The escrow contract trusts `oracle_admin` — the oracle service account
    // that the off-chain oracle uses to submit results.
    let escrow = EscrowContractClient::new(&env, &escrow_id);
    escrow.initialize(&oracle_admin, &admin);

    // Disable vesting so payouts can be claimed immediately in the test; the
    // default protocol config vests winnings over 3 days.
    escrow.set_protocol_config(&escrow::types::ProtocolConfig {
        vesting_duration_seconds: 0,
        cancellation_fee_basis_points: 0,
        treasury: admin.clone(),
        stablecoin_only_mode: false,
        maximum_stake: None,
        match_timeout_seconds: escrow::DEFAULT_MATCH_TIMEOUT_SECONDS,
        protocol_fee_bps: 0,
        fee_recipient: admin.clone(),
        minimum_stake: escrow::DEFAULT_MINIMUM_STAKE,
        max_protocol_fee: None,
        dispute_bond_tier_schedule: soroban_sdk::vec![&env],
    });

    let oracle = OracleContractClient::new(&env, &oracle_id);
    oracle.initialize(&oracle_admin);

    World {
        env,
        escrow,
        oracle,
        token,
        token_client,
        oracle_admin,
        admin,
        player1,
        player2,
    }
}

// ── Mock Chess.com API Server & Oracle Poller ────────────────────────────────

use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// Possible finished game outcomes reported by the Chess.com API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChessComOutcome {
    /// White won (`"end": {"result": "white"}` / `"white": {"result": "win"}`).
    WhiteWin,
    /// Black won (`"end": {"result": "black"}` / `"black": {"result": "win"}`).
    BlackWin,
    /// The game ended in a draw (`"end": {"result": "draw"}` / `"white": {"result": "agreed"}`).
    Draw,
}

impl ChessComOutcome {
    /// Map the Chess.com game outcome to the escrow contract's [`escrow::types::Winner`].
    pub fn to_escrow_winner(self) -> escrow::types::Winner {
        match self {
            ChessComOutcome::WhiteWin => escrow::types::Winner::Player1,
            ChessComOutcome::BlackWin => escrow::types::Winner::Player2,
            ChessComOutcome::Draw => escrow::types::Winner::Draw,
        }
    }

    /// Map the Chess.com game outcome to the oracle contract's [`oracle::types::Winner`].
    pub fn to_oracle_winner(self) -> oracle::types::Winner {
        match self {
            ChessComOutcome::WhiteWin => oracle::types::Winner::Player1,
            ChessComOutcome::BlackWin => oracle::types::Winner::Player2,
            ChessComOutcome::Draw => oracle::types::Winner::Draw,
        }
    }
}

/// Lifecycle state of a game hosted on the [`MockChessComServer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockChessComGameState {
    /// The game is currently being played on Chess.com and has no final result yet.
    InProgress,
    /// The game has finished with a final outcome.
    Finished(ChessComOutcome),
}

#[derive(Default)]
struct MockChessComShared {
    games: HashMap<std::string::String, MockChessComGameState>,
    request_counts: HashMap<std::string::String, usize>,
}

/// A lightweight local HTTP server that mocks the Chess.com public API
/// (`GET /pub/game/{game_id}` and `GET /pub/games/{ids_csv}`).
///
/// Mirrors the Lichess mock setup used in oracle integration tests, allowing
/// E2E tests to register an in-progress Chess.com game, transition it to a
/// finished state (`WhiteWin`, `BlackWin`, or `Draw`), and let the oracle
/// poller fetch and submit the result on-chain.
pub struct MockChessComServer {
    addr: SocketAddr,
    shared: Arc<Mutex<MockChessComShared>>,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl MockChessComServer {
    /// Start a new mock Chess.com HTTP server bound to an ephemeral localhost port.
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind MockChessComServer");
        let addr = listener.local_addr().expect("failed to read local_addr");
        let shared = Arc::new(Mutex::new(MockChessComShared::default()));
        let shutdown = Arc::new(AtomicBool::new(false));

        let shared_bg = Arc::clone(&shared);
        let shutdown_bg = Arc::clone(&shutdown);
        let handle = thread::spawn(move || {
            for stream in listener.incoming() {
                if shutdown_bg.load(Ordering::SeqCst) {
                    break;
                }
                if let Ok(mut stream) = stream {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    Self::handle_connection(&mut stream, &shared_bg);
                }
            }
        });

        Self {
            addr,
            shared,
            shutdown,
            handle: Some(handle),
        }
    }

    /// Base URL of the mock Chess.com server (e.g. `"http://127.0.0.1:54321"`).
    pub fn base_url(&self) -> std::string::String {
        format!("http://{}", self.addr)
    }

    /// Socket address of the running mock server.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Register a Chess.com game as currently in progress (not yet finished).
    pub fn register_in_progress(&self, game_id: &str) {
        let mut guard = self.shared.lock().expect("mock state lock poisoned");
        guard
            .games
            .insert(game_id.to_string(), MockChessComGameState::InProgress);
    }

    /// Mark a Chess.com game as finished with the given [`ChessComOutcome`].
    pub fn finish_game(&self, game_id: &str, outcome: ChessComOutcome) {
        let mut guard = self.shared.lock().expect("mock state lock poisoned");
        guard.games.insert(
            game_id.to_string(),
            MockChessComGameState::Finished(outcome),
        );
    }

    /// Return the number of HTTP requests the mock server has received for `game_id`.
    pub fn request_count(&self, game_id: &str) -> usize {
        let guard = self.shared.lock().expect("mock state lock poisoned");
        guard.request_counts.get(game_id).copied().unwrap_or(0)
    }

    fn format_game_json(game_id: &str, state: MockChessComGameState) -> std::string::String {
        match state {
            MockChessComGameState::InProgress => format!(
                r#"{{"id":"{game_id}","url":"https://www.chess.com/game/live/{game_id}","status":"in_progress","end":null,"white":{{"result":""}},"black":{{"result":""}}}}"#
            ),
            MockChessComGameState::Finished(outcome) => {
                let (end_result, white_result, black_result) = match outcome {
                    ChessComOutcome::WhiteWin => ("white", "win", "checkmated"),
                    ChessComOutcome::BlackWin => ("black", "resigned", "win"),
                    ChessComOutcome::Draw => ("draw", "agreed", "agreed"),
                };
                format!(
                    r#"{{"id":"{game_id}","url":"https://www.chess.com/game/live/{game_id}","status":"finished","end":{{"result":"{end_result}"}},"white":{{"result":"{white_result}"}},"black":{{"result":"{black_result}"}}}}"#
                )
            }
        }
    }

    fn handle_connection(stream: &mut TcpStream, shared: &Arc<Mutex<MockChessComShared>>) {
        let mut buf = [0u8; 4096];
        let n = match stream.read(&mut buf) {
            Ok(n) if n > 0 => n,
            _ => return,
        };
        let req = std::str::from_utf8(&buf[..n]).unwrap_or("");
        let request_line = req.lines().next().unwrap_or("");
        let path = request_line.split_whitespace().nth(1).unwrap_or("");

        let (status_line, body) = if let Some(game_id) = path.strip_prefix("/pub/game/") {
            let game_id = game_id.trim_end_matches('/');
            let mut guard = shared.lock().expect("mock state lock poisoned");
            *guard.request_counts.entry(game_id.to_string()).or_insert(0) += 1;
            match guard.games.get(game_id).copied() {
                Some(state) => ("HTTP/1.1 200 OK", Self::format_game_json(game_id, state)),
                None => (
                    "HTTP/1.1 404 Not Found",
                    r#"{"message":"Game not found"}"#.to_string(),
                ),
            }
        } else if let Some(ids_csv) = path.strip_prefix("/pub/games/") {
            let ids_csv = ids_csv.trim_end_matches('/');
            let mut guard = shared.lock().expect("mock state lock poisoned");
            let mut entries = Vec::new();
            for id in ids_csv.split(',').filter(|s| !s.is_empty()) {
                *guard.request_counts.entry(id.to_string()).or_insert(0) += 1;
                if let Some(&state) = guard.games.get(id) {
                    entries.push(Self::format_game_json(id, state));
                }
            }
            (
                "HTTP/1.1 200 OK",
                format!(r#"{{"games":[{}]}}"#, entries.join(",")),
            )
        } else {
            (
                "HTTP/1.1 404 Not Found",
                r#"{"message":"Endpoint not found"}"#.to_string(),
            )
        };

        let response = format!(
            "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    }

    /// Perform an HTTP `GET /pub/game/{game_id}` request against this mock
    /// Chess.com server and parse the outcome in the same way as the oracle
    /// service's `ChessComClient`.
    ///
    /// Returns:
    /// - `Ok((Some(outcome), response_time_ms))` when the game has finished,
    /// - `Ok((None, response_time_ms))` when the game is still in progress or 404,
    /// - `Err(reason)` if `game_id` is not a valid Chess.com numeric ID or the HTTP call fails.
    pub fn fetch_game_result(
        &self,
        game_id: &str,
    ) -> Result<(Option<ChessComOutcome>, u64), std::string::String> {
        if !is_valid_chess_com_game_id(game_id) {
            return Err(format!("invalid Chess.com game id: {game_id}"));
        }

        let start = Instant::now();
        let mut stream = TcpStream::connect(self.addr)
            .map_err(|e| format!("failed to connect to mock Chess.com server: {e}"))?;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));

        let request = format!(
            "GET /pub/game/{game_id} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nConnection: close\r\n\r\n",
            self.addr
        );
        stream
            .write_all(request.as_bytes())
            .map_err(|e| format!("failed to send request to mock Chess.com server: {e}"))?;
        let _ = stream.shutdown(Shutdown::Write);

        let mut response = std::string::String::new();
        stream
            .read_to_string(&mut response)
            .map_err(|e| format!("failed to read response from mock Chess.com server: {e}"))?;
        let elapsed_ms = start.elapsed().as_millis().max(1) as u64;

        let (headers, body) = response
            .split_once("\r\n\r\n")
            .ok_or_else(|| "malformed HTTP response from mock Chess.com server".to_string())?;
        let status_line = headers.lines().next().unwrap_or("");

        if status_line.contains("404") {
            return Ok((None, elapsed_ms));
        }
        if !status_line.contains("200") {
            return Err(format!(
                "unexpected HTTP status from Chess.com API: {status_line}"
            ));
        }

        if body.contains(r#""end":null"#) || body.contains(r#""status":"in_progress""#) {
            return Ok((None, elapsed_ms));
        }
        if body.contains(r#""end":{"result":"white"}"#) {
            return Ok((Some(ChessComOutcome::WhiteWin), elapsed_ms));
        }
        if body.contains(r#""end":{"result":"black"}"#) {
            return Ok((Some(ChessComOutcome::BlackWin), elapsed_ms));
        }
        if body.contains(r#""end":{"result":"draw"}"#) {
            return Ok((Some(ChessComOutcome::Draw), elapsed_ms));
        }

        Err(format!("unrecognized Chess.com response payload: {body}"))
    }
}

impl Drop for MockChessComServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Validate that `game_id` is a valid Chess.com game identifier (7–12 ASCII digits).
pub fn is_valid_chess_com_game_id(game_id: &str) -> bool {
    let len = game_id.len();
    (7..=12).contains(&len) && game_id.chars().all(|c| c.is_ascii_digit())
}

fn soroban_string_to_std(s: &soroban_sdk::String) -> std::string::String {
    let len = s.len() as usize;
    let mut buf = vec![0u8; len];
    s.copy_into_slice(&mut buf);
    std::string::String::from_utf8(buf).expect("on-chain game_id must be valid UTF-8")
}

/// Oracle poller harness that drives the Chess.com polling path against a
/// [`MockChessComServer`] and submits verified outcomes to the deployed
/// `oracle` and `escrow` contracts in [`World`].
pub struct ChessComPoller<'a> {
    server: &'a MockChessComServer,
    confidence: Option<u8>,
}

impl<'a> ChessComPoller<'a> {
    /// Create a new Chess.com oracle poller pointed at `server`.
    pub fn new(server: &'a MockChessComServer, confidence: Option<u8>) -> Self {
        Self { server, confidence }
    }

    /// Poll a single match by `match_id`.
    ///
    /// - Verifies the match is `Active` and uses `Platform::ChessDotCom`.
    /// - Queries the [`MockChessComServer`] over HTTP (`GET /pub/game/{game_id}`).
    /// - If the game is still in progress on Chess.com, leaves the on-chain
    ///   match `Active` and returns `Ok(false)`.
    /// - Once the game finishes, records the result in the `oracle` contract
    ///   and settles the match in the `escrow` contract, returning `Ok(true)`.
    pub fn poll_match(
        &self,
        world: &World<'_>,
        match_id: u64,
    ) -> Result<bool, std::string::String> {
        let m = world.escrow.get_match(&match_id);
        if m.state != escrow::types::MatchState::Active {
            return Ok(false);
        }
        if m.platform != escrow::types::Platform::ChessDotCom {
            return Ok(false);
        }
        if world.oracle.has_result(&match_id) {
            return Ok(false);
        }

        let game_id = soroban_string_to_std(&m.game_id);
        let (outcome, response_time_ms) = self.server.fetch_game_result(&game_id)?;
        let Some(outcome) = outcome else {
            return Ok(false);
        };

        let oracle_winner = outcome.to_oracle_winner();
        let escrow_winner = outcome.to_escrow_winner();

        world.oracle.submit_result(
            &match_id,
            &m.game_id,
            &oracle::types::Platform::ChessDotCom,
            &oracle_winner,
            &response_time_ms,
            &self.confidence,
        );
        world.escrow.submit_result(
            &match_id,
            &escrow_winner,
            &world.oracle_admin,
            &self.confidence,
        );

        Ok(true)
    }

    /// Run one full Chess.com reconciliation + verification tick across all
    /// active matches on the escrow contract.
    ///
    /// Returns the IDs of all Chess.com matches that were settled during this tick.
    pub fn tick(&self, world: &World<'_>) -> Result<Vec<u64>, std::string::String> {
        let active_matches = world.escrow.get_active_matches();
        let mut settled_ids = Vec::new();
        for m in active_matches.iter() {
            if m.platform == escrow::types::Platform::ChessDotCom
                && !world.oracle.has_result(&m.id)
                && self.poll_match(world, m.id)?
            {
                settled_ids.push(m.id);
            }
        }
        Ok(settled_ids)
    }
}
