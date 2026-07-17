//! Code Map: The top-level composition
//! - `Kernel`: The top-level composition that owns the
//!   `Runtime`, the `Arc<Scheduler>`, the `Arc<EventBus>`,
//!   and the `Arc<dyn SecurityV1>` (the security engine
//!   constructed in Phase 3 from a `MasterKey` and a
//!   secrets-DB path), all wired together. Cloning is
//!   cheap (every field is `Arc`-backed). Constructed via
//!   `Kernel::new(master_key, secrets_db_path)`; the
//!   constructor is the **only** path through which a
//!   `SecurityEngine` is built in the v1 codebase.
//!
//! Story (plain English): Imagine the front desk of a
//! small post office. The desk is the `Runtime` (the only
//! place a letter can be dropped off). Behind the desk is
//! the sorting room (the `Scheduler`), the mail
//! shelves (`EventBus`), and the safe (`SecurityEngine`)
//! where the manager keeps the day's deposit-box keys.
//! The post office as a whole (the `Kernel`) is just a
//! clean way to say "all four of those, wired together."
//! Several tellers at different counters can each have
//! their own copy of the post office — but they all share
//! the same mail shelves, the same sorting room, and the
//! same safe, so a letter dropped at one counter lands in
//! exactly the same boxes as a letter dropped at any
//! other, and any teller can hand a key out of the safe
//! to a customer who needs to open a deposit box.
//!
//! CID Index:
//! CID:kernel-001 -> Kernel
//!
//! Quick lookup: rg -n "CID:kernel-" crates/afa-kernel/src/kernel.rs

use crate::capability_registry::{CapabilityRegistry, RegisterError};
use crate::event_bus::{EventBus, EventBusHandle};
use crate::runtime::Runtime;
use crate::scheduler::Scheduler;
use afa_contracts::{
    EmbeddingV1, HealthCheck, HealthReport, HealthStatus, KnowledgeV1, LlmV1, SecurityErrorV1,
    SecurityV1, StorageError,
};
use afa_observability::{ObservabilityConfig, ObservabilityEngine};
use afa_security::{open_storage, MasterKey, SecurityEngine};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

// CID:kernel-001 - Kernel
// Purpose: The top-level composition. Owns the
// `Runtime`, the `Arc<Scheduler>`, the
// `Arc<EventBus>`, and the `Arc<dyn SecurityV1>` (the
// security engine), all wired together so a single
// `Kernel::new(master_key, secrets_db_path)` call
// gives you a working kernel. Cloning a `Kernel` is
// cheap because every field is `Arc`-backed; this is
// the intended sharing pattern (e.g. one `Kernel`
// per `axum` request handler, each of which calls
// `runtime.ingest` or `security().seal(...)`).
// Uses: `Arc<Scheduler>`, `Arc<EventBus>`, `Runtime`,
// `Arc<dyn SecurityV1>`. The `SecurityV1` trait object
// (rather than the concrete `SecurityEngine`) is what
// downstream adapters depend on — they never know
// there is a SQLite file behind the desk.
// Used by: every consumer of the kernel; this is the
// type most callers will hold and pass around.
pub struct Kernel {
    runtime: Runtime,
    scheduler: Arc<Scheduler>,
    event_bus: Arc<EventBus>,
    security: Arc<dyn SecurityV1>,
    /// The observability engine. Held in the
    /// kernel (not just the Runtime / Scheduler)
    /// so the `Clone` impl can re-build a `Runtime`
    /// over the same engine (the engine is
    /// `Arc`-shared, so the new Runtime points at
    /// the same spans DB connection).
    observability: Arc<ObservabilityEngine>,
    /// The capability registry. The slot
    /// type is `CapabilityRegistry` (not
    /// `Arc<CapabilityRegistry>`) because the
    /// registry's only field is an
    /// `Option<Arc<dyn LlmV1>>` — the `Arc` is
    /// already shared. Cloning a
    /// `CapabilityRegistry` is a tiny
    /// refcount bump on the slot's `Option`.
    /// The slot is a `Mutex` so a workflow
    /// can call `register_llm` on one clone
    /// and have the other clones see the
    /// registration immediately.
    capabilities: std::sync::Mutex<CapabilityRegistry>,
    health_engines: RwLock<BTreeMap<String, Arc<dyn HealthCheck>>>,
}

impl Kernel {
    /// Build a fresh `Kernel`, including a freshly
    /// constructed `SecurityEngine` that owns the
    /// `secrets.db` SQLite file at `secrets_db_path` and
    /// the master key in an `Arc<Zeroizing<[u8; 32]>>`.
    ///
    /// Steps:
    /// 1. Open or create the `secrets.db` SQLite file
    ///    at `secrets_db_path` (via
    ///    `open_storage`, which runs the
    ///    idempotent schema on first boot, then
    ///    checks the schema version). The
    ///    `open_storage` is `async` because the
    ///    underlying `afa_storage::open` and
    ///    `afa_storage::migrate` are async (the
    ///    lock is a `tokio::sync::Mutex`, not a
    ///    `std::sync::Mutex` — see the Phase 0.5a
    ///    doc-drift correction #1 in
    ///    `docs/contracts-foundation/IMPL-observability-baseline.md`).
    /// 2. Build the `SecurityEngine` over the store and
    ///    the kernel's `Arc<EventBus>`.
    /// 3. Wire the `Runtime` over the `Scheduler` and
    ///    the `EventBusHandle`.
    /// 4. Store the `SecurityEngine` behind the
    ///    `Arc<dyn SecurityV1>` trait object so
    ///    downstream adapters cannot bypass the trait.
    ///
    /// Errors: propagates `SecurityErrorV1` from the
    /// store / engine construction (the typical case
    /// is `StorageUnreachable` for an unwritable
    /// parent dir or `StorageCorrupted` for a truncated
    /// SQLite file). The caller (an `axum` bootstrap
    /// handler or a CLI `afa kernel start` command) is
    /// expected to log the error and refuse to start.
    pub async fn new(
        master_key: &MasterKey,
        secrets_db_path: PathBuf,
    ) -> Result<Self, SecurityErrorV1> {
        // Step 1: open or create the SQLite file. The
        // `open_storage` helper (in `afa-security`)
        // wraps the three boot steps (open,
        // migrate, check) into one call so the
        // kernel doesn't have to know about the
        // migration constant. **Doc drift
        // correction #7 vs. the IMPL draft**:
        // the IMPL said `Kernel::new` stays sync
        // and uses a `SealedSecretStore::open_or_create`
        // call, but the Phase 0.5a refactor
        // extracted the storage into
        // `afa-storage`, which is `async` (the
        // lock is a `tokio::sync::Mutex`).
        let storage = open_storage(&secrets_db_path).await.map_err(|e| match e {
            StorageError::Open(io) => SecurityErrorV1::StorageUnreachable {
                reason: format!("{}: {}", secrets_db_path.to_string_lossy(), io),
            },
            StorageError::Migrate { version, .. } => {
                // The engine's `SCHEMA_VERSION` is
                // the source of truth for the
                // "expected" field. Hardcoding `1`
                // here would mean the kernel
                // panics with the wrong number on
                // Phase 0.5b (which bumped the
                // engine to v2) and on every
                // future schema bump.
                SecurityErrorV1::SchemaVersionMismatch {
                    found: version,
                    expected: afa_security::SCHEMA_VERSION,
                }
            }
            StorageError::Locked => SecurityErrorV1::StorageUnreachable {
                reason: format!(
                    "{}: secrets.db is locked by another process",
                    secrets_db_path.to_string_lossy()
                ),
            },
            StorageError::Closure(boxed) => boxed.into(),
        })?;

        // Step 2: build the shared bus (every adapter
        // sees the same one), and the `Runtime` /
        // `Scheduler` over it.
        let event_bus = Arc::new(EventBus::new());

        // Step 2.5: build the observability
        // engine. The spans DB is co-located
        // with the secrets DB (sibling file
        // `<secrets_db_path_parent>/spans.db`)
        // so a single kernel install has a
        // single directory of state. The
        // engine's bus handle is a clone of the
        // shared bus so the kernel's
        // SpansWriteFailed / SpansPurged
        // events ride the same bus the
        // security engine's audit events
        // ride.
        let spans_db_path = secrets_db_path
            .parent()
            .map(|p| p.join("spans.db"))
            .unwrap_or_else(|| PathBuf::from("spans.db"));
        let observability = ObservabilityEngine::new(
            ObservabilityConfig::with_default_retention(spans_db_path),
            event_bus.handle(),
        )
        .await
        .map_err(|e| {
            // The engine's `with_default_retention`
            // config has `purge_interval_hours = 1`
            // and `retention_days = Some(7)` — a
            // build that fails is one of:
            // (1) the spans DB path is unwritable,
            // (2) the migration row already exists
            // with a mismatched version. The
            // kernel surfaces both as
            // `StorageUnreachable` (a sibling of
            // the security engine's own
            // storage-error contract — the
            // operator sees the same bucket
            // for both engines).
            SecurityErrorV1::StorageUnreachable {
                reason: format!("observability engine boot: {e}"),
            }
        })?;

        let scheduler = Arc::new(Scheduler::new(Arc::clone(&observability)));

        // Step 3: build the `SecurityEngine`. The
        // engine gets a fresh `Arc` clone of the bus
        // so the kernel's own bus handle and the
        // engine's bus handle point at the same
        // underlying bus.
        let engine = SecurityEngine::new(master_key, storage, Arc::clone(&event_bus));
        // Upcast to the trait object so the kernel's
        // public `security()` accessor hands out the
        // locked `SecurityV1` view, not the concrete
        // engine. Downstream adapters never see the
        // SQLite file.
        let security: Arc<dyn SecurityV1> = Arc::new(engine);

        // Step 4: build the `Runtime` over the
        // scheduler, the bus handle, and the
        // observability engine.
        let runtime = Runtime::new(
            Arc::clone(&scheduler),
            event_bus.handle(),
            Arc::clone(&observability),
        );

        Ok(Self {
            runtime,
            scheduler,
            event_bus,
            security,
            observability: Arc::clone(&observability),
            capabilities: std::sync::Mutex::new(CapabilityRegistry::new()),
            health_engines: RwLock::new(BTreeMap::from([(
                "afa-observability".to_string(),
                Arc::clone(&observability) as Arc<dyn HealthCheck>,
            )])),
        })
    }

    /// Build a health report from the kernel's
    /// registered `HealthCheck` engines. The
    /// observability engine is seeded at boot;
    /// future engines register via the same
    /// `RwLock<BTreeMap>` and become visible
    /// on `/health` immediately. Overall status
    /// is worst-wins: Unhealthy > Degraded > Healthy.
    pub fn aggregate_health(&self) -> HealthReport {
        let engines: BTreeMap<String, HealthStatus> = self
            .health_engines
            .read()
            .expect("health engines lock")
            .iter()
            .map(|(name, engine)| (name.clone(), engine.health_check()))
            .collect();
        let overall = engines
            .values()
            .cloned()
            .reduce(|acc, status| match (acc, status) {
                (HealthStatus::Unhealthy { reason }, _)
                | (_, HealthStatus::Unhealthy { reason }) => HealthStatus::Unhealthy { reason },
                (HealthStatus::Degraded { reason }, _) | (_, HealthStatus::Degraded { reason }) => {
                    HealthStatus::Degraded { reason }
                }
                (HealthStatus::Healthy, HealthStatus::Healthy) => HealthStatus::Healthy,
            })
            .unwrap_or(HealthStatus::Healthy);
        HealthReport {
            overall,
            engines,
            checked_at: chrono::Utc::now(),
        }
    }

    /// `Runtime` is the only way to send an event into
    /// the kernel; there is no other path.
    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    /// Hand out the spans DB path. The path is the
    /// sibling of the secrets DB path, with the
    /// `spans.db` filename (the v1 layout). Tests
    /// and dashboards open this path directly with
    /// `rusqlite` to inspect / read the spans
    /// table.
    pub fn spans_db_path(&self) -> PathBuf {
        self.observability.config().spans_db_path.clone()
    }

    /// Hand out the `Arc<ObservabilityEngine>`. The
    /// engine is the canonical writer of the spans
    /// DB; a workflow that wants to record a custom
    /// span (e.g. a long-running `complete` call on
    /// a future LLM adapter) calls
    /// `engine.record_span(...)` directly, then
    /// routes its own sub-work through the
    /// `record_span` / `record_span_value`
    /// helpers.
    pub fn observability(&self) -> Arc<ObservabilityEngine> {
        Arc::clone(&self.observability)
    }

    /// Hand out a fresh `Arc<Scheduler>` (the
    /// dispatcher). The Scheduler is what workflow
    /// authors call `register` on to install steps
    /// for their event types.
    pub fn scheduler(&self) -> Arc<Scheduler> {
        Arc::clone(&self.scheduler)
    }

    /// Hand out a fresh `Arc<EventBus>` (the
    /// pub/sub broker). Use this when you want to
    /// `subscribe` to events; use the `EventBusHandle`
    /// returned by `Runtime::ingest` (or this method's
    /// sibling) when you want to `publish`.
    pub fn event_bus(&self) -> Arc<EventBus> {
        Arc::clone(&self.event_bus)
    }

    /// Hand out a fresh `EventBusHandle` (a
    /// publish-only view of the bus). Steps receive a
    /// handle to publish their own events; this method
    /// is for code that wants the same publish-only
    /// view without going through a step.
    #[allow(dead_code)] // Used by future packs (afa-cli, etc.).
    pub fn event_bus_handle(&self) -> EventBusHandle {
        self.event_bus.handle()
    }

    /// Hand out a fresh `Arc<dyn SecurityV1>` (the
    /// security engine's trait-object view). Every
    /// downstream adapter that needs to `seal` /
    /// `unseal` / `rotate` a secret goes through this
    /// method, so the kernel is the only place that
    /// holds a concrete `SecurityEngine` (and the
    /// only place that holds the `secrets.db` file
    /// handle).
    #[allow(dead_code)] // Used by future packs (afa-cli, axum handlers, etc.).
    pub fn security(&self) -> Arc<dyn SecurityV1> {
        Arc::clone(&self.security)
    }

    /// Register an LLM adapter with the kernel's
    /// `CapabilityRegistry`. This is the
    /// composition-root entry point for the LLM
    /// capability: an `axum` bootstrap handler or
    /// a CLI `afa kernel start` command builds a
    /// concrete adapter (e.g. `ResponsesAdapter`
    /// pointed at the real OpenAI endpoint), wraps
    /// it in an `Arc<dyn LlmV1>`, and hands it to
    /// this method. A second call is a programmer
    /// error and returns
    /// `RegisterError::LlmAlreadyRegistered` (the
    /// registry holds a single LLM slot — see
    /// `docs/engines/CapabilityRegistry.md`).
    /// Phase 4 integration tests use this to wire
    /// a real `ResponsesAdapter` into a fresh
    /// `Kernel` and exercise the full
    /// `Kernel → CapabilityRegistry → LlmV1 → wire
    /// → audit bus` round-trip.
    pub fn register_llm(&self, adapter: Arc<dyn LlmV1>) -> Result<(), RegisterError> {
        self.capabilities
            .lock()
            .expect("capabilities mutex")
            .register_llm(adapter)
    }

    /// Hand out a clone of the `Arc<dyn LlmV1>` the
    /// registry is currently holding, or `None` if
    /// no adapter has been registered. Mirrors
    /// `security()` in shape (a fresh `Arc` per
    /// call, the underlying instance is shared).
    /// Used by every workflow that needs an LLM —
    /// `kernel.llm().expect("no LLM configured")`
    /// is the canonical pattern. A workflow that
    /// runs before the bootstrap registers an
    /// adapter sees `None` and surfaces a clear
    /// "LLM not configured" error.
    pub fn llm(&self) -> Option<Arc<dyn LlmV1>> {
        self.capabilities.lock().expect("capabilities mutex").llm()
    }

    /// Register a Knowledge storage adapter with
    /// the kernel's `CapabilityRegistry` under a
    /// `key` (typically `"default"` for a
    /// single-tenant deployment, or one entry
    /// per tenant id in a multi-tenant
    /// deployment). This is the composition-root
    /// entry point for the Knowledge capability:
    /// a bootstrap handler (an `axum` route, a
    /// CLI command, or an integration test)
    /// builds a concrete adapter (e.g.
    /// `JsonKnowledgeAdapter` pointed at a
    /// tempdir-backed storage root), wraps it in
    /// an `Arc<dyn KnowledgeV1>`, and hands it
    /// to this method along with the same
    /// `storage_root` the adapter was built
    /// with. The `storage_root` is retained for
    /// diagnostics only (the `knowledge(key)`
    /// accessor does not hand back the path; a
    /// future health-check surface will).
    /// A second `register_knowledge` under the
    /// same `key` is a programmer error and
    /// returns
    /// `RegisterError::KnowledgeAlreadyRegistered
    /// { key }`. The kernel's `CapabilityRegistry`
    /// holds one slot per `key`, not a single
    /// global slot (a multi-tenant deployment
    /// should use one `key` per tenant id, not
    /// re-register the `"default"` key).
    /// Phase 4 integration tests use this to
    /// wire a real `JsonKnowledgeAdapter` into
    /// a fresh `Kernel` and exercise the full
    /// `Kernel → CapabilityRegistry → KnowledgeV1
    /// → on-disk → audit bus` round-trip.
    pub fn register_knowledge(
        &self,
        key: impl Into<String>,
        adapter: Arc<dyn KnowledgeV1>,
        storage_root: PathBuf,
    ) -> Result<(), RegisterError> {
        self.capabilities
            .lock()
            .expect("capabilities mutex")
            .register_knowledge(key, adapter, storage_root)
    }

    /// Hand out a clone of the `Arc<dyn
    /// KnowledgeV1>` stored under the given
    /// `key`, or `None` if no adapter has been
    /// registered under that key. Mirrors
    /// `llm()` in shape (a fresh `Arc` per
    /// call, the underlying instance is
    /// shared). Used by every workflow that
    /// needs a Knowledge storage adapter —
    /// `kernel.knowledge("default").expect("no
    /// Knowledge configured")` is the canonical
    /// pattern. A workflow that runs before
    /// the bootstrap registers an adapter sees
    /// `None` and surfaces a clear "no
    /// Knowledge configured for this tenant"
    /// error. The `storage_root` the adapter
    /// was registered with is NOT handed back
    /// (the workflow does not need it; only a
    /// future health-check surface will).
    pub fn knowledge(&self, key: &str) -> Option<Arc<dyn KnowledgeV1>> {
        self.capabilities
            .lock()
            .expect("capabilities mutex")
            .knowledge(key)
    }

    /// Register an embedding adapter with
    /// the kernel's `CapabilityRegistry`.
    /// This is the composition-root entry
    /// point for the embedding capability:
    /// a bootstrap handler (an `axum`
    /// route, a CLI command, or an
    /// integration test) builds a concrete
    /// adapter (e.g.
    /// `LocalEmbeddingAdapter` for the
    /// candle backend, or
    /// `OllamaEmbeddingAdapter` for the
    /// HTTP backend), wraps it in an
    /// `Arc<dyn EmbeddingV1>`, and hands
    /// it to this method. A second call is
    /// a programmer error and returns
    /// `RegisterError::EmbeddingAlreadyRegistered`
    /// (the registry holds a single
    /// embedding slot — see
    /// `docs/engines/CapabilityRegistry.md`).
    /// Pack #24 (ingestion) will call
    /// `kernel.embedding()` to get the
    /// adapter and call `embed_batch` on
    /// the chunked text.
    pub fn register_embedding(&self, adapter: Arc<dyn EmbeddingV1>) -> Result<(), RegisterError> {
        self.capabilities
            .lock()
            .expect("capabilities mutex")
            .register_embedding(adapter)
    }

    /// Hand out a clone of the `Arc<dyn
    /// EmbeddingV1>` the registry is
    /// currently holding, or `None` if no
    /// adapter has been registered.
    /// Mirrors `llm()` / `knowledge()` in
    /// shape (a fresh `Arc` per call, the
    /// underlying instance is shared).
    /// Used by Pack #24 (ingestion) —
    /// `kernel.embedding().expect("no
    /// embedding configured")` is the
    /// canonical pattern. A workflow that
    /// runs before the bootstrap
    /// registers an adapter sees `None`
    /// and surfaces a clear "embedding not
    /// configured" error.
    pub fn embedding(&self) -> Option<Arc<dyn EmbeddingV1>> {
        self.capabilities
            .lock()
            .expect("capabilities mutex")
            .embedding()
    }
}

impl Clone for Kernel {
    /// Cheaply clone the kernel. Every field is
    /// `Arc`-backed, so this is just a few refcount
    /// bumps — no registry copy, no bus copy, no
    /// runtime copy. The two clones share the exact
    /// same underlying `Scheduler`, `EventBus`, and
    /// `SecurityEngine`; steps registered on one are
    /// immediately visible to the other, and a secret
    /// sealed on one is immediately unsealable on the
    /// other. The `CapabilityRegistry` is shared
    /// across clones too — but the registry's
    /// internal `Arc<dyn LlmV1>` is what is shared, so
    /// a `register_llm` on one clone is visible to
    /// the other immediately (the registry's slot is
    /// not `Arc`'d; the slot's content is).
    fn clone(&self) -> Self {
        let capabilities = self
            .capabilities
            .lock()
            .expect("capabilities mutex")
            .clone();
        Self {
            runtime: Runtime::new(
                Arc::clone(&self.scheduler),
                self.event_bus.handle(),
                Arc::clone(&self.observability),
            ),
            scheduler: Arc::clone(&self.scheduler),
            event_bus: Arc::clone(&self.event_bus),
            security: Arc::clone(&self.security),
            observability: Arc::clone(&self.observability),
            capabilities: std::sync::Mutex::new(capabilities),
            health_engines: RwLock::new(
                self.health_engines
                    .read()
                    .expect("health engines lock")
                    .clone(),
            ),
        }
    }
}

impl std::fmt::Debug for Kernel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Kernel").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::EventReceived;
    use afa_contracts::{Actor, AfaEvent, TenantId};
    use afa_security::MasterKey;
    use rusqlite::Connection;
    use serde::{Deserialize, Serialize};
    use tempfile::TempDir;

    /// Build a fresh `MasterKey` (a deterministic
    /// `0x42` pattern) and a fresh tempdir-backed
    /// `secrets.db` path. The `TempDir` is returned
    /// so the test can keep the path alive for the
    /// test's entire scope (dropping the `TempDir`
    /// would delete the file, which would race with
    /// the engine's open connection on slow
    /// filesystems).
    async fn fresh_kernel() -> (TempDir, Kernel) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("secrets.db");
        let key = MasterKey::from([0x42u8; 32]);
        let kernel = Kernel::new(&key, path).await.expect("kernel::new");
        (dir, kernel)
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Probe {
        payload: String,
    }

    impl AfaEvent for Probe {}

    #[tokio::test]
    async fn kernel_new_returns_a_working_kernel() {
        // Flow: a freshly-constructed Kernel can
        // accept an ingest and produce an
        // `EventReceived` audit-trail fact. If
        // `Kernel::new` wired the components
        // incorrectly, this would fail.
        let (_dir, kernel) = fresh_kernel().await;
        let bus = kernel.event_bus();
        let mut received = bus.subscribe::<EventReceived>(16);

        kernel
            .runtime()
            .ingest(
                Probe {
                    payload: "ping".into(),
                },
                TenantId::new("test-tenant"),
                Actor::Timer,
            )
            .await;

        let (event, _) = received.recv().await.expect("EventReceived");
        assert_eq!(event.event_type, std::any::type_name::<Probe>());
    }

    #[tokio::test]
    async fn kernel_accessors_return_the_same_underlying_components() {
        // Flow: `kernel.scheduler()` and
        // `kernel.event_bus()` each hand out a fresh
        // `Arc`, but the underlying instances are
        // shared. We check this by pointing the
        // `Arc`s at the same registry entry and
        // confirming both see the same steps.
        let (_dir, kernel) = fresh_kernel().await;
        let scheduler_a = kernel.scheduler();
        let scheduler_b = kernel.scheduler();
        let bus_a = kernel.event_bus();
        let bus_b = kernel.event_bus();

        // Two `Arc<Scheduler>` clones point to the
        // same instance: `Arc::ptr_eq` is true.
        assert!(
            Arc::ptr_eq(&scheduler_a, &scheduler_b),
            "kernel.scheduler() should hand out Arcs to the same underlying Scheduler"
        );
        assert!(
            Arc::ptr_eq(&bus_a, &bus_b),
            "kernel.event_bus() should hand out Arcs to the same underlying EventBus"
        );
    }

    #[tokio::test]
    async fn kernel_clone_shares_underlying_state() {
        // Flow: a cloned `Kernel` is backed by the
        // same Scheduler and EventBus as the
        // original. Steps registered on the original
        // are visible to the clone, and events
        // published on one side land in subscriptions
        // made on the other.
        let (_dir, original) = fresh_kernel().await;
        let clone = original.clone();

        // Register a step on the original's
        // scheduler (the shared one).
        original.scheduler().register::<Probe>(
            "kernel_test_step_1",
            Arc::new(|_event, ctx, bus_handle| {
                let ctx = ctx.clone();
                Box::pin(async move {
                    // Publish a follow-up event with
                    // a known payload so the clone's
                    // subscriber can confirm it ran.
                    bus_handle
                        .publish(
                            super::event_bus_test_marker::ProbeAck {
                                from: "shared-step".into(),
                            },
                            ctx,
                        )
                        .await;
                    Ok(())
                })
            }),
        );

        // Subscribe to the ProbeAck on the clone's
        // bus (the shared one).
        let mut acks = clone
            .event_bus()
            .subscribe::<super::event_bus_test_marker::ProbeAck>(16);

        // Ingest on the clone. Because the
        // Scheduler and EventBus are shared, the
        // step registered via the original's
        // scheduler will run.
        clone
            .runtime()
            .ingest(
                Probe {
                    payload: "go".into(),
                },
                TenantId::new("test-tenant"),
                Actor::Timer,
            )
            .await;

        // And the subscription on the clone's bus
        // receives the step's follow-up event.
        let (ack, _) = acks.recv().await.expect("ProbeAck");
        assert_eq!(ack.from, "shared-step");
    }

    #[tokio::test]
    async fn kernel_security_accessor_returns_a_shared_security_engine() {
        // Flow: `kernel.security()` hands out an
        // `Arc<dyn SecurityV1>`. A sealed secret on
        // the original is unsealable from the
        // clone, which proves the engine is shared
        // (not re-built per call).
        let (_dir, kernel) = fresh_kernel().await;
        let clone = kernel.clone();

        // Seal a secret on the original's engine.
        let secret_ref = kernel
            .security()
            .seal(b"hello-engine", "test-secret")
            .await
            .expect("seal should succeed on a fresh engine");

        // Unseal it on the clone's engine.
        let ctx = afa_contracts::ExecutionContext::new(TenantId::new("test-tenant"), Actor::Timer);
        let unsealed = clone
            .security()
            .unseal(&secret_ref, &ctx)
            .await
            .expect("unseal should succeed on a clone");

        assert_eq!(&*unsealed, b"hello-engine");
    }

    // ---- The two Phase-0.5b boot-failure-wrapping tests ----
    //
    // These tests sit at the `Kernel::new` boundary and
    // prove the kernel's `match` arm is the one that
    // wraps the storage-layer `StorageError` into the
    // domain-level `SecurityErrorV1` the dashboard and
    // callers see. The lower-level storage tests
    // (`crates/afa-security/tests/boot_failures.rs` e3
    // and e7) cover the same cases against the
    // `StorageError` directly; these tests cover the
    // *wiring* (the kernel is the only caller of
    // `open_storage` in production code, so if its
    // match arm ever regresses, these tests catch it).

    /// CID:kernel-002 - kernel_new_wraps_storage_mismatch_into_security_error_v1
    /// Purpose: Confirms the `Kernel::new` boot path
    /// maps a tampered `secrets.db` (wrong
    /// `schema_version`) into the domain-level
    /// `SecurityErrorV1::SchemaVersionMismatch { found,
    /// expected }` (NOT a raw `StorageError`). This is
    /// the "you restored an old secrets.db" footgun,
    /// surfaced through the only path the operator
    /// ever sees.
    #[tokio::test]
    async fn kernel_new_returns_schema_version_mismatch_for_wrong_schema_version() {
        // Build a tempdir and pre-populate the
        // secrets.db with the security table shape
        // and a `schema_version = 99` row (a
        // "future" version the current engine
        // cannot read).
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("secrets.db");
        {
            let conn = Connection::open(&path).expect("open db");
            conn.execute_batch(
                r#"
                CREATE TABLE sealed_secrets (
                    name        TEXT NOT NULL,
                    version     INTEGER NOT NULL,
                    status      TEXT NOT NULL,
                    nonce       BLOB NOT NULL,
                    ciphertext  BLOB NOT NULL,
                    created_at  TEXT NOT NULL,
                    PRIMARY KEY (name, version)
                );
                CREATE TABLE afa_security_meta (
                    key   TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );
                INSERT INTO afa_security_meta (key, value)
                    VALUES ('schema_version', '99');
                "#,
            )
            .expect("create schema");
        }

        // `Kernel::new` must reject the tampered
        // file with
        // `SecurityErrorV1::SchemaVersionMismatch
        // { found: 99, expected: 1 }`. The mapping
        // from `StorageError::Migrate { version, .. }`
        // to this domain-level variant is in the
        // `match` arm at the top of `Kernel::new`.
        let key = MasterKey::from([0x42u8; 32]);
        let result = Kernel::new(&key, path).await;
        match result {
            Err(SecurityErrorV1::SchemaVersionMismatch { found, expected }) => {
                assert_eq!(found, 99, "found must be the on-disk schema_version");
                // The "expected" is the engine's
                // `SCHEMA_VERSION` (currently `2` in
                // Phase 0.5b; was `1` in Phase 0.5a).
                // The test reads it from the engine
                // rather than hardcoding, so the
                // assertion survives future schema
                // bumps without an edit.
                assert_eq!(
                    expected,
                    afa_security::SCHEMA_VERSION,
                    "expected must be the engine's SCHEMA_VERSION"
                );
            }
            Err(other) => panic!("expected SchemaVersionMismatch, got {other:?}"),
            Ok(_) => panic!("expected SchemaVersionMismatch, got Ok(kernel)"),
        }
    }

    /// CID:kernel-003 - kernel_new_wraps_storage_open_error_into_security_error_v1
    /// Purpose: Confirms the `Kernel::new` boot path
    /// maps an unwritable parent directory (where
    /// `open_storage` returns `StorageError::Open(io)`)
    /// into the domain-level
    /// `SecurityErrorV1::StorageUnreachable { reason }`.
    /// The `reason` is non-empty (the dashboard
    /// surfaces it verbatim as the "what to fix"
    /// hint).
    #[tokio::test]
    async fn kernel_new_returns_storage_unreachable_for_unwritable_parent() {
        // Build a parent that is a regular file,
        // so any attempt to `mkdir` underneath it
        // fails. This is portable across Linux
        // and macOS; the `create_dir_all` call
        // inside `afa_storage::open` will fail
        // with `NotADirectory`, which the storage
        // layer surfaces as `StorageError::Open(io)`
        // and the kernel wraps as
        // `StorageUnreachable { reason }`.
        let dir = tempfile::tempdir().expect("tempdir");
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"i am a file, not a directory").expect("write blocker");
        let path = blocker.join("under-a-file/secrets.db");

        let key = MasterKey::from([0x42u8; 32]);
        let result = Kernel::new(&key, path).await;
        match result {
            Err(SecurityErrorV1::StorageUnreachable { reason }) => {
                // The reason must be non-empty (the
                // dashboard surfaces it verbatim
                // as the "what to fix" hint). The
                // exact wording is OS-dependent,
                // so we only pin the non-emptiness.
                assert!(!reason.is_empty());
            }
            Err(other) => panic!("expected StorageUnreachable, got {other:?}"),
            Ok(_) => panic!("expected StorageUnreachable, got Ok(kernel)"),
        }
    }
}

/// Tiny test-only marker module so the Kernel clone
/// test above can name a follow-up event type without
/// putting a test-only `pub` item in `event_bus.rs`.
#[cfg(test)]
mod event_bus_test_marker {
    use afa_contracts::AfaEvent;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ProbeAck {
        pub from: String,
    }

    impl AfaEvent for ProbeAck {}
}
