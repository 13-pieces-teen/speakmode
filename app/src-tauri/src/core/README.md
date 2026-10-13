# SpeakMode domain/storage

`types.rs` defines Serde camelCase domain DTOs. `store.rs` owns parameterized SQLite migrations and data invariants; it does not own audio, network requests, keyboard hooks, target validation or IPC authorization.

## Integration contract

- Production uses `DbClient`: one named OS thread opens, migrates, recovers, and exclusively owns `Store`. The synchronous Store API remains for isolated storage tests. The engine must never await a DB ticket or native ACK while holding its control/publication mutex.
- `CommitGuard` arbitrates the actual final COMMIT plus MemoryOnly publication. The caller supplies the matching session/generation guard. `cancel_unfinished` records durable cleanup, while the nonblocking guard controls accepted/queued/in-flight commits. Cleanup is never rejected merely because cancellation was accepted.
- `checkpoint_source(seed, source)` accepts only complete, ordered, unique Final items of the frozen attempt. It writes source/body metadata, no Usage or output. Memory-only policy keeps these bodies only in memory.
- `commit_ready(output)` writes initial Output/current pointer and independent Usage atomically. Memory-only Ready commits only Usage to SQLite and publishes its prepared in-memory result inside the same short guard; actor/cache observers see them after the combined boundary. It is idempotent for the identical initial revision.
- `use_source(session_id, expected_generation, created_at)` adopts the original Final texts unchanged, commits one Usage and returns `creationReason=useSource`. It cannot receive automatic delivery permission.
- Pending save IDs, errors before checkpoint, partial copy data and ASR retry audio are coordinator-owned. A failed checkpoint leaves no source rows; the coordinator must keep its pending immutable source to retry.
- Retry callers must set `creationReason=retry`; the store rejects automatic delivery for retry/useSource/history revisions. It cannot infer whether a caller's normalInitial operation was retried.
- Command `requestId` deduplication and candidate/run ownership belong in the command/coordinator layer. Version operations check persisted `expectedCurrentRevisionId`; they do not cache IPC request IDs.
- Preference registration/system effects must succeed before `save_preferences` is called. The store verifies data/CAS, not OS hook or autostart success.
- End dates/time zones and monotonic capture duration are frozen by the coordinator. Use canonical UTC ISO strings for wall-clock timestamps so history sort order is chronological.

## Public APIs

`Store::open(path)` / `new(path)` / `in_memory()`; `recover_after_restart()` explicitly marks checkpointed sessions interrupted and both Prepared/Dispatched delivery unconfirmed. A write-ahead row cannot prove that no native side effect happened before a crash. Opening alone does not mutate recovery states.

Preferences: `preferences`, `save_preferences(next, expected_version)`, `reset_preferences(expected_version)`.

Provider: `provider_config`, `save_provider_config(config, expected_version)`. No Key/URL is a DTO field; configured is not a network-readiness assertion.

Modes: `list_modes`, `mode_snapshot(id, optional_revision)`, `save_mode(ModeDraft)`, `delete_mode(id, expected_revision)`, `set_default_mode(id, expected_preferences_version)`. Builtins are virtual/read-only; personal revisions remain after soft deletion. Normalized duplicate names use NFKC and full Unicode case folding.

Sources/results: `checkpoint_source`, `commit_ready`, `use_source`, `get_source`, `get_output`, `get_result`, `latest_temporary_result`, `clear_temporary_results`. Temporary current/one-level undo data is retained in memory; a new checkpoint does not replace the previous completed temporary result. New Ready replaces it.

History: `history_list(query, cursor_offset, limit)`, `history_get`, `delete_history`. Literal Unicode-normalized search is parameterized; maximum 50 entries. Checkpoint/interrupted source-only records also appear. Deletion removes sources/items/versions but leaves Usage/opaque delivery rows and revokes Prepared delivery.

Versions: `adopt_candidate(session, expected_revision, candidate)`, `edit_output(session, expected_revision, blocks, created_at)`, `restore_output(session, expected_revision, RestorePatch, created_at)`, `undo_output(session, expected_revision, created_at)`. All return `HistoryDetail`. Source references must belong to the bound immutable source revision. Restore uses explicit user-selected Unicode scalar ranges; it does not infer reliable semantic alignment.

From 0.2.18, the engine's explicit history editor returns a transient candidate plus review findings. `revision.adopt` may carry the user's `reviewedText`; edits are bound to the cached candidate's complete source set, without inferring ASR ranges from display diffs. Candidate ID, unchanged base, source identity and cancellation guards remain authoritative. Human adoption never grants automatic delivery permission. See the [review plan](../../../../docs/plans/candidate-review-0.2.18.md).

Usage: `usage_event(session_id)` / `usage_today(local_date)`. Unicode source count excludes punctuation/whitespace/symbols/controls and counts letters/numbers/marks; actual source text is authoritative. Version changes, copies, deleted history and delivery status do not change statistics.

Delivery: `prepare_delivery(attempt)` write-ahead commits one opaque permission per session. It checks Ready/normalInitial/current output/generation/Usage; caller separately checks direct preference, target and cancel/quit immediately before dispatch. `transition_delivery(attempt_id, expected_state, next_state, reason_code)` refuses terminal reset/reuse.

## DB actor API and operation ownership

`DbClient::open(path)` / `in_memory()` perform synchronous bootstrap only: the dedicated worker opens SQLite, runs migrations, applies `recover_after_restart`, and installs one consistent snapshot before returning. Failure keeps the existing file and returns an error; no usable client or default-zero substitute is produced.

- `Clone`, `snapshot() -> DbSnapshot`, `cache_error() -> Option<StoreError>`, `outstanding()`, `queued_bytes()`, and `owner_released()` contain no SQL. Snapshot holds only preferences, provider config, Mode list, dated usage and the current temporary result.
- `submit(DbCommand, Option<Arc<CommitGuard>>, DbPriority) -> StoreResult<DbTicket>` uses `try_send`. Defaults: normal queue 32, critical queue 8, serialized-body budget 8 MiB. JSON byte counting uses a counting writer, not an extra serialized body allocation. Payload ownership remains reserved through real execution, cache publication and completion.
- CancelUnfinished has its own bounded one-slot lane, independent of ordinary/critical queues and body budget. A duplicate cancellation of the same active session shares its actual receipt; a second distinct cleanup identity returns Busy. This is a reservation for the engine's single active session, not an unlimited cancellation queue. IDs have a separate 2048-byte envelope bound. The engine must not start another active session while old DB/session resources are outstanding.
- After four critical/control selections, an already-queued normal job is served. Shutdown uses an independent wake signal rather than jumping ahead of accepted SQL work.
- Tickets are cloneable observers over one outcome (`Mutex` + `Condvar` + Tokio `Notify`). `wait().await` and `wait_blocking()` observe the same result. Aborting/dropping a waiter cannot cancel the accepted operation or release its budget. Engine receipt owners must still apply the eventual result to their matching session/run; this actor cannot enforce caller lock discipline.
- `request_shutdown()` closes admission and wakes the worker. `submit(Shutdown, None, Critical).wait()` and `shutdown().await` wait for every accepted job, temporary-body clearing and actual Store/connection drop. `owner_released()` is set only after that drop. Last-client disconnection also drains and releases the owner.
- Production content writes (Checkpoint/Ready/UseSource/Adopt/Edit/Restore/Undo/PrepareDelivery) require an identity-matching guard. Cleanup/actual delivery-receipt recording is not blocked by a generic cancel gate. Settings and Mode writes use the supplied operation guard at final COMMIT too.
- `DbValue::Mode(ModeRecord)` is the SaveMode result. Arbitrary SQL/Count commands are compiled only under `cfg(test)` and are not an IPC API.

`CommitGuard::new(session_id, generation)` is the record guard. `cancel()` first records intent, then uses `try_lock`; it returns Accepted, Pending, or AlreadyCompleted. A checkpoint commit never marks Ready. `delete()` is permanent shared intent. `operation()` creates a child for one candidate/run with independent cancellation/completion but the same record deletion/commit gate. A sibling operation holding the gate does not leave an unrelated cancelled child permanently Pending. A child's successful first adoption marks the parent Ready; cancelling a completed record's delivery cannot invalidate later history editing. The worker recognizes already-persisted Ready records before applying a newly-created history guard.

Store checks the guard at final COMMIT, after SQL and JSON work; it does not hold it during BEGIN, INSERTs, searches or serialization. MemoryOnly replacement and its Usage commit publish in that same gate. Successful write DTOs are prepared before commit, so a subsequent cache query cannot turn a successful commit into a reported write failure. Read commands and compound snapshots use an explicit consistent read transaction.

All explicit transactions use checked rollback on early errors and failed COMMIT. `StoreError::Busy` is based on SQLite's typed Busy/Locked codes, never string parsing. Rollback/connection-state failure marks Unhealthy, stops admission, and preserves an error outcome; it cannot be reported as cleanup success. The production busy timeout remains 2 seconds per SQLite wait, not an operation-wide deadline or a promise that final cancellation cleanup is instantaneous.

Cache refresh errors are separate from successful write outcomes. The last valid dated cache is retained and `cache_error()` exposes staleness. Idle refresh checks calendar-date changes; `ReadSnapshot { date }` explicitly loads a date without inventing zero statistics. Cleared or superseded MemoryOnly plaintext is immediately redacted even when an unrelated preference/Mode query prevents a full refresh. Existing known counters remain stale and labelled by date/error, rather than being replaced with zeroes.

## Test scope and current validation

The clean repository uses the production Cargo library test target. The earlier
isolated harness and its machine-local receipts are not included. Historical
module results are not a claim that the current exported source has passed.
See [repository validation](../../../../docs/validation.md) for the current run
status and the boundary of retained historical evidence.

Run from the repository's `app/` directory on Windows with Rust/MSVC and the
desktop build prerequisites available:

```powershell
cargo test --locked --manifest-path src-tauri/Cargo.toml --features desktop --target x86_64-pc-windows-msvc --lib
cargo test --locked --manifest-path src-tauri/Cargo.toml --features desktop --target x86_64-pc-windows-msvc --lib core:: -- --test-threads=4
```

The second command filters the same library to core/storage tests; it does not
use a copied implementation. Database fixtures use test-owned paths, separate
from the running application profile. These tests use actual second SQLite
connections and rollback-journal read/write locks. They cover queue/byte
exhaustion, reserved coalesced cleanup, observer abort, final COMMIT contention
and typed Busy rollback, checkpoint-versus-Ready cancellation, parent/child
cancellation and deletion, source-only adoption/unique Usage, MemoryOnly table
privacy and cache redaction, settings cancellation, Unicode restoration/CAS/undo,
cache-failure receipts/date refresh, failed migration preservation, accepted-work
shutdown drain and SQLite handle release after owner exit or last-client drop.
The Busy test changes only its test connection timeout to 300 ms; production
remains 2 seconds.

Storage/actor tests do not establish native stop latency, cloud/audio delivery,
real profile upgrade, release/installer acceptance, or complete application
lifecycle acceptance. These require their own integration or user-task evidence.
The earlier checks did not inject unrecoverable disk/rollback I/O faults; checked
rollback handling must not be presented as such a fault-injection result.
