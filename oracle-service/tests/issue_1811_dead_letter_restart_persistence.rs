//! Tests for issue #1811: Dead-letter queue persists across service restarts.
//!
//! The dead-letter store is backed by `{queue_dir}/dead_letter.json` and uses
//! atomic write-then-rename to prevent corruption on crash.  These tests verify
//! that:
//!
//! 1. Entries written by one `DeadLetterStore` handle are fully readable by a
//!    brand-new handle pointing at the same directory (simulating a restart).
//! 2. All fields (`match_id`, `game_id`, `platform`, `attempts`, `last_error`,
//!    `dead_lettered_at`, `total_attempts`) survive the round-trip through JSON.
//! 3. Multiple entries all survive the restart, in the same order.
//! 4. Entries present after a restart are still recognised as eligible for
//!    reconciliation (i.e. `PendingQueue::enqueue` accepts them after
//!    `DeadLetterStore::remove` is called, mirroring the replay flow).
//! 5. An empty store (no file on disk yet) loads cleanly on a fresh handle.
//! 6. The store survives two consecutive "restarts" (three handles total).
//! 7. Capacity (`max_entries`) is preserved and enforced correctly after a
//!    restart.

use tempfile::TempDir;

use oracle_service::{
    config::Platform,
    dead_letter::DeadLetterStore,
    queue::{PendingEntry, PendingQueue},
};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build a `PendingEntry` that looks like it has already failed `attempts`
/// times and carries a `last_error` string, so that when it is dead-lettered
/// `total_attempts` equals the supplied value.
fn make_failed_entry(match_id: u64, game_id: &str, platform: Platform, attempts: u32) -> PendingEntry {
    let mut e = PendingEntry::new(match_id, game_id.to_string(), platform);
    e.attempts = attempts;
    e.last_error = Some(format!("simulated failure after {} attempts", attempts));
    e
}

/// Construct a `DeadLetterStore` backed by `dir` with no capacity limit.
fn store(dir: &TempDir) -> DeadLetterStore {
    DeadLetterStore::new(dir.path().to_str().unwrap(), 0)
}

/// Construct a `DeadLetterStore` backed by `dir` with a capacity limit.
fn store_capped(dir: &TempDir, max: usize) -> DeadLetterStore {
    DeadLetterStore::new(dir.path().to_str().unwrap(), max)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// A single dead-lettered entry survives dropping and re-creating the store
/// handle (simulating a service restart).
#[tokio::test]
async fn single_entry_survives_restart() {
    let dir = TempDir::new().unwrap();

    // --- "pre-restart" handle ---
    {
        let s = store(&dir);
        s.push(make_failed_entry(1, "abcd1234", Platform::Lichess, 5))
            .await
            .unwrap();
        // Handle drops here — no in-memory state survives.
    }

    // --- "post-restart" handle: constructed from scratch, same directory ---
    let s2 = store(&dir);
    let entries = s2.load().await.unwrap();

    assert_eq!(entries.len(), 1, "entry should survive restart");
    let e = &entries[0];
    assert_eq!(e.entry.match_id, 1);
    assert_eq!(e.entry.game_id, "abcd1234");
    assert_eq!(e.entry.platform, Platform::Lichess);
    assert_eq!(e.total_attempts, 5);
    assert_eq!(
        e.entry.last_error.as_deref(),
        Some("simulated failure after 5 attempts")
    );
}

/// All fields in `DeadLetterEntry` / `PendingEntry` survive JSON
/// serialisation and deserialisation through a restart.
#[tokio::test]
async fn all_fields_survive_round_trip() {
    let dir = TempDir::new().unwrap();

    let before_restart = {
        let s = store(&dir);
        let entry = make_failed_entry(42, "xyz98765432", Platform::ChessDotCom, 3);
        s.push(entry).await.unwrap();

        // Capture the entry as written so we can compare after reload.
        let written = s.load().await.unwrap();
        assert_eq!(written.len(), 1);
        written.into_iter().next().unwrap()
    };
    // Handle dropped here.

    let s2 = store(&dir);
    let after_restart = s2.load().await.unwrap();

    assert_eq!(after_restart.len(), 1);
    let e = &after_restart[0];

    // Core identity fields
    assert_eq!(e.entry.match_id,  before_restart.entry.match_id);
    assert_eq!(e.entry.game_id,   before_restart.entry.game_id);
    assert_eq!(e.entry.platform,  before_restart.entry.platform);
    assert_eq!(e.entry.attempts,  before_restart.entry.attempts);
    assert_eq!(e.entry.last_error, before_restart.entry.last_error);
    assert_eq!(e.total_attempts,  before_restart.total_attempts);

    // Timestamps must survive JSON (chrono::DateTime<Utc> with serde feature)
    assert_eq!(e.dead_lettered_at,        before_restart.dead_lettered_at);
    assert_eq!(e.entry.enqueued_at,       before_restart.entry.enqueued_at);
    assert_eq!(e.entry.next_attempt_at,   before_restart.entry.next_attempt_at);
}

/// Multiple entries all survive the restart, and their order is preserved.
#[tokio::test]
async fn multiple_entries_survive_restart() {
    let dir = TempDir::new().unwrap();

    let ids: Vec<u64> = vec![10, 20, 30, 40, 50];

    {
        let s = store(&dir);
        for &id in &ids {
            s.push(make_failed_entry(id, &format!("game{:04}", id), Platform::Lichess, 5))
                .await
                .unwrap();
        }
    }
    // Handle dropped.

    let s2 = store(&dir);
    let entries = s2.load().await.unwrap();

    assert_eq!(entries.len(), ids.len(), "all entries should survive");

    let loaded_ids: Vec<u64> = entries.iter().map(|e| e.entry.match_id).collect();
    assert_eq!(
        loaded_ids, ids,
        "entries should be in insertion order after restart"
    );
}

/// After a restart, entries are still eligible for reconciliation: removing
/// one from the dead-letter store and re-enqueuing it via `PendingQueue`
/// (the manual replay flow) works correctly on the reloaded handle.
#[tokio::test]
async fn entries_eligible_for_reconciliation_after_restart() {
    let dir = TempDir::new().unwrap();
    let dir_str = dir.path().to_str().unwrap();

    // Populate and drop the original handle.
    {
        let s = DeadLetterStore::new(dir_str, 0);
        s.push(make_failed_entry(100, "abcd1234", Platform::Lichess, 5))
            .await
            .unwrap();
        s.push(make_failed_entry(101, "efgh5678", Platform::Lichess, 5))
            .await
            .unwrap();
    }

    // Simulate restart: brand-new store and queue handles.
    let s2   = DeadLetterStore::new(dir_str, 0);
    let queue = PendingQueue::new(dir_str);

    let dead_entries = s2.load().await.unwrap();
    assert_eq!(dead_entries.len(), 2, "both entries survive restart");

    // Replay match 100: remove from DLQ, re-enqueue in pending queue.
    s2.remove(100).await.unwrap();
    let added = queue
        .enqueue(100, "abcd1234".to_string(), Platform::Lichess)
        .await
        .unwrap();
    assert!(added, "re-enqueue should succeed after removal from DLQ");

    // match 101 must still be in the DLQ.
    let remaining = s2.load().await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].entry.match_id, 101);

    // Pending queue should now hold the replayed entry.
    let pending = queue.load().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].match_id, 100);
}

/// A fresh store handle on a directory that has no `dead_letter.json` yet
/// returns an empty list — it does not error.
#[tokio::test]
async fn empty_store_loads_cleanly_after_restart() {
    let dir = TempDir::new().unwrap();

    // No writes at all — the file does not exist.
    let s = store(&dir);
    let entries = s.load().await.unwrap();
    assert!(
        entries.is_empty(),
        "loading a non-existent DLQ file should return empty vec, not error"
    );
}

/// The store survives two consecutive restarts (three distinct handles),
/// with new entries added between each restart.
#[tokio::test]
async fn survives_two_consecutive_restarts() {
    let dir = TempDir::new().unwrap();
    let dir_str = dir.path().to_str().unwrap();

    // First "boot": write entry 1.
    {
        let s = DeadLetterStore::new(dir_str, 0);
        s.push(make_failed_entry(1, "game0001", Platform::Lichess, 5))
            .await
            .unwrap();
    }

    // Second "boot": write entry 2, verify entry 1 is still there.
    {
        let s = DeadLetterStore::new(dir_str, 0);
        let loaded = s.load().await.unwrap();
        assert_eq!(loaded.len(), 1, "entry 1 present after first restart");
        assert_eq!(loaded[0].entry.match_id, 1);

        s.push(make_failed_entry(2, "game0002", Platform::ChessDotCom, 3))
            .await
            .unwrap();
    }

    // Third "boot": both entries must be present.
    let s = DeadLetterStore::new(dir_str, 0);
    let entries = s.load().await.unwrap();
    assert_eq!(entries.len(), 2, "both entries present after second restart");

    let ids: Vec<u64> = entries.iter().map(|e| e.entry.match_id).collect();
    assert!(ids.contains(&1), "entry 1 present");
    assert!(ids.contains(&2), "entry 2 present");
}

/// After a restart the capacity (`max_entries`) is still enforced: if the
/// reloaded store is already at its limit, pushing a new entry still evicts
/// the oldest one.
#[tokio::test]
async fn capacity_enforced_after_restart() {
    let dir = TempDir::new().unwrap();
    let dir_str = dir.path().to_str().unwrap();

    // Fill to capacity=2 and drop the handle.
    {
        let s = store_capped(&dir, 2);
        s.push(make_failed_entry(1, "game0001", Platform::Lichess, 5))
            .await
            .unwrap();
        s.push(make_failed_entry(2, "game0002", Platform::Lichess, 5))
            .await
            .unwrap();
    }

    // New handle (post-restart) — still capped at 2.
    let s2 = store_capped(&dir, 2);

    // Confirm both entries loaded.
    let loaded = s2.load().await.unwrap();
    assert_eq!(loaded.len(), 2, "two entries loaded after restart");

    // Push a third entry — the oldest (match_id=1) must be evicted.
    s2.push(make_failed_entry(3, "game0003", Platform::Lichess, 5))
        .await
        .unwrap();

    let after = s2.load().await.unwrap();
    assert_eq!(after.len(), 2, "store respects capacity after restart");

    let ids: Vec<u64> = after.iter().map(|e| e.entry.match_id).collect();
    assert!(
        !ids.contains(&1),
        "oldest entry evicted after capacity-limited push post-restart; ids={:?}",
        ids
    );
    assert!(ids.contains(&3), "new entry present; ids={:?}", ids);
}

/// The idempotency guarantee (`push` for the same `match_id` is a no-op)
/// holds across a restart: re-pushing an already dead-lettered entry after
/// re-creating the handle must not create a duplicate.
#[tokio::test]
async fn push_idempotent_across_restart() {
    let dir = TempDir::new().unwrap();
    let dir_str = dir.path().to_str().unwrap();

    {
        let s = DeadLetterStore::new(dir_str, 0);
        s.push(make_failed_entry(99, "abcd1234", Platform::Lichess, 5))
            .await
            .unwrap();
    }

    // Post-restart: push the same match_id again — must remain exactly 1 entry.
    let s2 = DeadLetterStore::new(dir_str, 0);
    s2.push(make_failed_entry(99, "abcd1234", Platform::Lichess, 5))
        .await
        .unwrap();

    let entries = s2.load().await.unwrap();
    assert_eq!(
        entries.len(),
        1,
        "duplicate push across restart must not create a second entry"
    );
}
