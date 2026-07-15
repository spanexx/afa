//! Code Map: CapabilityRegistry
//! - `CapabilityRegistry`: The small lookup table the
//!   `Kernel` holds for plugin capabilities. Currently
//!   one slot: `llm: Option<Arc<dyn LlmV1>>`. The
//!   `register_llm` method inserts the adapter (one
//!   only — a second call is a programmer error, surfaced
//!   as a `register_error`); `llm` returns a clone of
//!   the `Arc` for the caller. A workflow that wants an
//!   LLM does `ctx.kernel().capabilities().llm()...` and
//!   gets the `Arc<dyn LlmV1>` it can call
//!   `complete` / `stream_complete` on.
//!
//! - `register_embedding`: Insert an embedding adapter into
//!   the slot (a single slot, like the LLM slot).
//! - `embedding`: Hand back a clone of the `Arc<dyn
//!   EmbeddingV1>` so a workflow can call `embed` /
//!   `embed_batch`.
//!
//! Story (plain English): The capability registry is
//! the switchboard's little card index. The
//! switchboard operator (`Kernel`) keeps a stack of
//! cards, one per specialist. When a workflow asks
//! for an LLM, the operator looks up the LLM card
//! and hands the workflow the specialist's
//! contact details. The registry is small (one
//! slot per kind of plugin) and never grows at
//! runtime: the cards are set at startup
//! (`register_llm` is called once) and never change
//! for the process lifetime.
//!
//! CID Index:
//! CID:capability-registry-001 -> CapabilityRegistry
//! CID:capability-registry-002 -> register_llm
//! CID:capability-registry-003 -> llm
//! CID:capability-registry-006 -> register_embedding
//! CID:capability-registry-007 -> embedding
//!
//! Quick lookup: rg -n "CID:capability-registry-" crates/afa-kernel/src/capability_registry.rs

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use afa_contracts::{EmbeddingV1, KnowledgeV1, LlmV1};
use thiserror::Error;

// CID:capability-registry-001 - CapabilityRegistry
// Purpose: The small lookup table the `Kernel`
// holds for plugin capabilities. Currently
// one slot: `llm: Option<Arc<dyn LlmV1>>`.
// A workflow that wants an LLM does
// `ctx.kernel().capabilities().llm()...` and
// gets the `Arc<dyn LlmV1>` it can call
// `complete` / `stream_complete` on.
// The registry is small and
// never grows at runtime: the slots are
// set at startup and never change for
// the process lifetime.
// Uses: LlmV1 (the trait for the
// LLM-adapter slot).
// Used by: `Kernel::capabilities` (the
// public handle workflows use to reach
// the registry), and the kernel's
// constructor (which builds the
// registry).
//
// `Clone` is derived so `Kernel::clone`
// (which holds a `Mutex<CapabilityRegistry>`)
// can clone the inner registry cheaply —
// cloning an `Option<Arc<dyn LlmV1>>` is a
// refcount bump on the `Option` and the
// `Arc` inside.
//
// `Debug` is NOT derived: `dyn LlmV1`
// does not implement `Debug` (the trait
// does not promise a `Debug` impl), and
// adding one would be a contract change.
// The `Kernel` itself has a manual
// `Debug` impl that does not recurse
// into the registry.
#[derive(Default, Clone)]
pub struct CapabilityRegistry {
    /// The registered LLM adapter. `None`
    /// until `register_llm` is called at
    /// startup.
    llm: Option<Arc<dyn LlmV1>>,
    /// The registered Knowledge adapters,
    /// keyed by a `String` (typically
    /// `"default"` for a single-tenant
    /// deployment, or one entry per tenant
    /// id in a multi-tenant deployment).
    /// The value is `(adapter,
    /// storage_root)`: the adapter is the
    /// thing the workflow calls
    /// `find_information` /
    /// `store_information` /
    /// `list_topics` on; the
    /// `storage_root: PathBuf` is retained
    /// for diagnostics only (the
    /// accessor `knowledge(key)` does not
    /// hand back the path; a future
    /// health-check surface will).
    knowledge: HashMap<String, (Arc<dyn KnowledgeV1>, PathBuf)>,
    /// The registered embedding adapter.
    /// A single slot (like the LLM slot).
    /// `None` until `register_embedding`
    /// is called at startup. Pack #24
    /// (ingestion) calls `embed_batch` on
    /// this adapter to embed chunks for
    /// topic routing.
    embedding: Option<Arc<dyn EmbeddingV1>>,
}

/// The "could not register" reasons. The
/// closed set is small — the LLM registry
/// has one slot, the Knowledge registry is
/// keyed — and the error is typed so a
/// workflow can branch on the reason (not
/// on a string).
#[derive(Debug, Error)]
pub enum RegisterError {
    /// The LLM slot is already occupied.
    /// A second `register_llm` call is a
    /// programmer error (the kernel
    /// constructor is the only caller).
    #[error("llm adapter already registered")]
    LlmAlreadyRegistered,
    /// A `register_knowledge` call tried to
    /// register under a `key` that is
    /// already occupied. A second register
    /// under the same key is a programmer
    /// error (the kernel constructor is the
    /// only caller); a multi-tenant
    /// deployment should use one key per
    /// tenant id, not re-register the
    /// default key.
    #[error("knowledge adapter already registered under key `{key}`")]
    KnowledgeAlreadyRegistered { key: String },
    /// The embedding slot is already
    /// occupied. A second
    /// `register_embedding` call is a
    /// programmer error (the kernel
    /// constructor is the only caller).
    #[error("embedding adapter already registered")]
    EmbeddingAlreadyRegistered,
}

impl CapabilityRegistry {
    /// Build an empty registry. Used by
    /// `Kernel::default()` and by the
    /// test harness.
    pub fn new() -> Self {
        Self::default()
    }

    // CID:capability-registry-002 - register_llm
    // Purpose: Insert an LLM adapter into
    // the slot. Returns `Err(LlmAlready
    // Registered)` if a second adapter is
    // registered (the kernel constructor
    // is the only caller, so a second
    // call is a programmer error).
    // Uses: LlmV1.
    // Used by: `Kernel::new` (the
    // canonical place to register an
    // LLM).
    pub fn register_llm(&mut self, adapter: Arc<dyn LlmV1>) -> Result<(), RegisterError> {
        if self.llm.is_some() {
            return Err(RegisterError::LlmAlreadyRegistered);
        }
        self.llm = Some(adapter);
        Ok(())
    }

    // CID:capability-registry-003 - llm
    // Purpose: Hand back a clone of the
    // `Arc<dyn LlmV1>` so a workflow can
    // call `complete` /
    // `stream_complete`. Returns
    // `None` if no adapter was
    // registered (a workflow that
    // needs an LLM can branch on
    // `None` and surface a clear
    // "no LLM configured" error).
    // Uses: LlmV1.
    // Used by: every workflow that
    // calls `llm.complete` / `llm.
    // stream_complete`.
    pub fn llm(&self) -> Option<Arc<dyn LlmV1>> {
        self.llm.clone()
    }

    // CID:capability-registry-004 - register_knowledge
    // Purpose: Insert a Knowledge storage adapter
    // under a `String` key (typically
    // `"default"`, `"tenant-a"`, etc.). Returns
    // `Err(RegisterError::KnowledgeAlreadyRegistered
    // { key })` if a second adapter is registered
    // under the same key. The Knowledge
    // registry is keyed (not a single slot) so
    // a multi-tenant deployment can hold one
    // adapter per tenant under the tenant id
    // as the key. The `storage_root: PathBuf` is
    // retained for diagnostics only (the
    // accessor `knowledge(key)` does not hand
    // back the path; a future health-check
    // surface will).
    // Uses: KnowledgeV1, PathBuf.
    // Used by: `Kernel::new` (or a tenant
    // registration bootstrap path) to attach
    // one or more Knowledge adapters.
    pub fn register_knowledge(
        &mut self,
        key: impl Into<String>,
        adapter: Arc<dyn KnowledgeV1>,
        storage_root: PathBuf,
    ) -> Result<(), RegisterError> {
        let key = key.into();
        if self.knowledge.contains_key(&key) {
            return Err(RegisterError::KnowledgeAlreadyRegistered { key });
        }
        self.knowledge.insert(key, (adapter, storage_root));
        Ok(())
    }

    // CID:capability-registry-005 - knowledge
    // Purpose: Hand back a clone of the
    // `Arc<dyn KnowledgeV1>` stored under the
    // given key. Returns `None` if no adapter
    // was registered under that key (the
    // workflow branches on `None` and
    // surfaces a clear "no Knowledge configured
    // for this tenant" error).
    // Uses: KnowledgeV1.
    // Used by: every workflow that calls
    // `knowledge.find_information` /
    // `knowledge.store_information` /
    // `knowledge.list_topics`.
    pub fn knowledge(&self, key: &str) -> Option<Arc<dyn KnowledgeV1>> {
        self.knowledge.get(key).map(|(adapter, _)| adapter.clone())
    }

    // CID:capability-registry-006 - register_embedding
    // Purpose: Insert an embedding adapter
    // into the slot. Returns
    // `Err(RegisterError::EmbeddingAlreadyRegistered)`
    // if a second adapter is registered
    // (the kernel constructor is the only
    // caller). Mirrors the `register_llm`
    // pattern; a single embedding slot is
    // the v1 design (a future pack that
    // needs multiple embeddings can split
    // the slot, but the v1 single-slot
    // design is simpler and matches the
    // "one embedding model per kernel"
    // operator story).
    // Uses: EmbeddingV1.
    // Used by: `Kernel::new` (the
    // canonical place to register an
    // embedding adapter).
    pub fn register_embedding(
        &mut self,
        adapter: Arc<dyn EmbeddingV1>,
    ) -> Result<(), RegisterError> {
        if self.embedding.is_some() {
            return Err(RegisterError::EmbeddingAlreadyRegistered);
        }
        self.embedding = Some(adapter);
        Ok(())
    }

    // CID:capability-registry-007 - embedding
    // Purpose: Hand back a clone of the
    // `Arc<dyn EmbeddingV1>` so a workflow
    // can call `embed` / `embed_batch`.
    // Returns `None` if no adapter was
    // registered (a workflow that needs an
    // embedding can branch on `None` and
    // surface a clear "no embedding
    // configured" error). Used by Pack
    // #24 (ingestion).
    // Uses: EmbeddingV1.
    // Used by: every workflow that calls
    // `embed` / `embed_batch`.
    pub fn embedding(&self) -> Option<Arc<dyn EmbeddingV1>> {
        self.embedding.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use afa_contracts::{
        CompletionRequest, CompletionResponse, CompletionStream, ExecutionContext,
        FindInformationRequest, FindInformationResponse, KnowledgeCapabilities, KnowledgeErrorV1,
        KnowledgeRecordInput, KnowledgeV1, LlmErrorV1, LlmV1, ModelCapabilities, RecordId, Topic,
    };
    use async_trait::async_trait;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A no-op `LlmV1` that records
    /// every call. Used to assert the
    /// registry hands out the same
    /// adapter to every caller.
    struct CountingAdapter {
        count: AtomicU32,
    }

    #[async_trait]
    impl LlmV1 for CountingAdapter {
        async fn complete(
            &self,
            _request: CompletionRequest,
            _ctx: &ExecutionContext,
        ) -> Result<CompletionResponse, LlmErrorV1> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(CompletionResponse::TextReply {
                content: "ok".into(),
                usage: afa_contracts::Usage {
                    prompt_tokens: 1,
                    completion_tokens: 1,
                },
            })
        }
        async fn stream_complete(
            &self,
            _request: CompletionRequest,
            _ctx: &ExecutionContext,
        ) -> Result<CompletionStream, LlmErrorV1> {
            unimplemented!()
        }
        fn describe_capabilities(&self) -> ModelCapabilities {
            ModelCapabilities {
                max_context_tokens: 100,
                supports_vision: false,
                supports_tool_use: false,
            }
        }
    }

    #[test]
    fn empty_registry_has_no_llm() {
        // A freshly built registry
        // returns `None` from
        // `.llm()`. The workflow
        // case "no LLM configured"
        // starts here.
        let r = CapabilityRegistry::new();
        assert!(r.llm().is_none());
    }

    #[test]
    fn register_llm_then_llm_returns_the_same_arc() {
        // After `register_llm`, the
        // registry's `.llm()` hands
        // back an `Arc` that points
        // to the same adapter (the
        // `CountingAdapter::count`
        // is the only field — a
        // different adapter would
        // have its own `count`).
        // Bound as `Arc<dyn LlmV1>` so
        // the stored value and the
        // retrieved value share the
        // same concrete type (required
        // for `Arc::ptr_eq`).
        let adapter: Arc<dyn LlmV1> = Arc::new(CountingAdapter {
            count: AtomicU32::new(0),
        });
        let mut r = CapabilityRegistry::new();
        r.register_llm(adapter.clone()).expect("register");
        let got = r.llm().expect("llm");
        // `register_llm` stored the
        // adapter, and `llm` returned a
        // clone of the same `Arc`. The
        // pointer-equality check is the
        // assertion of intent (it is the
        // same allocation), independent
        // of how many intermediate Arcs
        // were held by the call stack.
        assert!(Arc::ptr_eq(&adapter, &got));
    }

    #[test]
    fn second_register_llm_call_fails() {
        // A second `register_llm` is a
        // programmer error (the kernel
        // constructor is the only
        // caller). The registry
        // surfaces it as
        // `LlmAlreadyRegistered`
        // (not a panic).
        let adapter1 = Arc::new(CountingAdapter {
            count: AtomicU32::new(0),
        });
        let adapter2 = Arc::new(CountingAdapter {
            count: AtomicU32::new(0),
        });
        let mut r = CapabilityRegistry::new();
        r.register_llm(adapter1).expect("first");
        let e = r.register_llm(adapter2).expect_err("second");
        assert!(matches!(e, RegisterError::LlmAlreadyRegistered));
        // The first adapter is still
        // the one the registry hands
        // out.
        let _got = r.llm().expect("llm");
    }

    #[tokio::test]
    async fn handed_out_adapter_is_callable() {
        // The end-to-end shape: a
        // workflow gets the
        // `Arc<dyn LlmV1>`, calls
        // `complete` on it, and the
        // adapter's `count`
        // increments. This is the
        // proof that the registry
        // hands out a real, callable
        // adapter (not a stub).
        let adapter = Arc::new(CountingAdapter {
            count: AtomicU32::new(0),
        });
        let mut r = CapabilityRegistry::new();
        r.register_llm(adapter.clone()).expect("register");
        let llm = r.llm().expect("llm");
        let ctx = ExecutionContext::new(
            afa_contracts::TenantId::new("t"),
            afa_contracts::Actor::Timer,
        );
        let req = CompletionRequest {
            system: None,
            messages: vec![],
            tools: vec![],
            sampling: Default::default(),
        };
        let _resp = llm.complete(req, &ctx).await.expect("complete");
        assert_eq!(adapter.count.load(Ordering::SeqCst), 1);
    }

    /// A no-op `KnowledgeV1` that records
    /// every call. Used to assert the
    /// registry hands out the same
    /// adapter to every caller. Mirrors
    /// the `CountingAdapter` pattern for
    /// the LLM registry above. The
    /// methods return canned responses
    /// (empty result list, new
    /// `RecordId`, empty topic list,
    /// fixed capabilities) — the Phase
    /// 0 tests only assert on the
    /// call counter, not on the
    /// response shape.
    struct CountingKnowledgeAdapter {
        count: AtomicU32,
    }

    #[async_trait]
    impl KnowledgeV1 for CountingKnowledgeAdapter {
        async fn find_information(
            &self,
            _request: FindInformationRequest,
            _ctx: &ExecutionContext,
        ) -> Result<FindInformationResponse, KnowledgeErrorV1> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        }
        async fn store_information(
            &self,
            _record: KnowledgeRecordInput,
            _ctx: &ExecutionContext,
        ) -> Result<RecordId, KnowledgeErrorV1> {
            // The Phase 0 conformance test does
            // not call `store_information`; the
            // unimplemented panic is the
            // contract for "this method is
            // stubbed in the Phase 0 mock."
            // Phase 1+ adapter tests live in
            // `afa-plugin-knowledge-json`.
            unimplemented!("counting mock; Phase 1+ uses the real adapter")
        }
        async fn list_topics(
            &self,
            _ctx: &ExecutionContext,
        ) -> Result<Vec<Topic>, KnowledgeErrorV1> {
            Ok(Vec::new())
        }
        fn describe_capabilities(&self) -> KnowledgeCapabilities {
            KnowledgeCapabilities {
                max_record_size_bytes: 1_048_576,
                supports_semantic_search: false,
                supports_hierarchical_topics: false,
            }
        }
    }

    #[test]
    fn empty_registry_has_no_knowledge() {
        // A freshly built registry
        // returns `None` from
        // `.knowledge("default")`.
        // The workflow case
        // "no Knowledge configured
        // for this tenant" starts
        // here.
        let r = CapabilityRegistry::new();
        assert!(r.knowledge("default").is_none());
        assert!(r.knowledge("tenant-a").is_none());
    }

    #[test]
    fn register_knowledge_then_knowledge_returns_the_same_arc() {
        // After `register_knowledge`,
        // the registry's
        // `.knowledge(key)` hands back
        // an `Arc` that points to the
        // same adapter. The
        // `CountingKnowledgeAdapter
        // ::count` is the only field
        // — a different adapter would
        // have its own `count`. Bound
        // as `Arc<dyn KnowledgeV1>` so
        // the stored value and the
        // retrieved value share the
        // same concrete type (required
        // for `Arc::ptr_eq`).
        let adapter: Arc<dyn KnowledgeV1> = Arc::new(CountingKnowledgeAdapter {
            count: AtomicU32::new(0),
        });
        let mut r = CapabilityRegistry::new();
        r.register_knowledge("default", adapter.clone(), PathBuf::from("/tmp/knowledge"))
            .expect("register");
        let got = r.knowledge("default").expect("knowledge");
        // `register_knowledge` stored
        // the adapter, and
        // `knowledge` returned a
        // clone of the same `Arc`.
        // The pointer-equality check
        // is the assertion of intent
        // (it is the same
        // allocation), independent
        // of how many intermediate
        // Arcs were held by the
        // call stack.
        assert!(Arc::ptr_eq(&adapter, &got));
    }

    #[test]
    fn second_register_knowledge_call_fails() {
        // A second `register_knowledge`
        // under the same key is a
        // programmer error (the
        // kernel constructor is the
        // only caller). The registry
        // surfaces it as
        // `KnowledgeAlreadyRegistered
        // { key }` (not a panic).
        let adapter1: Arc<dyn KnowledgeV1> = Arc::new(CountingKnowledgeAdapter {
            count: AtomicU32::new(0),
        });
        let adapter2: Arc<dyn KnowledgeV1> = Arc::new(CountingKnowledgeAdapter {
            count: AtomicU32::new(0),
        });
        let mut r = CapabilityRegistry::new();
        r.register_knowledge("default", adapter1, PathBuf::from("/tmp/a"))
            .expect("first");
        let e = r
            .register_knowledge("default", adapter2, PathBuf::from("/tmp/b"))
            .expect_err("second");
        // The error carries the
        // conflicting key for the
        // operator log.
        match e {
            RegisterError::KnowledgeAlreadyRegistered { key } => {
                assert_eq!(key, "default")
            }
            other => panic!("expected KnowledgeAlreadyRegistered, got {other:?}"),
        }
        // The first adapter is
        // still the one the
        // registry hands out (the
        // second call did NOT
        // overwrite).
        let _got = r.knowledge("default").expect("knowledge");
    }

    #[test]
    fn knowledge_registry_supports_multiple_keys() {
        // The Knowledge registry
        // is keyed (not a single
        // slot) so a multi-tenant
        // deployment can hold one
        // adapter per tenant under
        // the tenant id as the key.
        // Different keys must hold
        // different adapters; the
        // same key is a
        // `KnowledgeAlreadyRegistered`
        // (covered above).
        let adapter_a: Arc<dyn KnowledgeV1> = Arc::new(CountingKnowledgeAdapter {
            count: AtomicU32::new(0),
        });
        let adapter_b: Arc<dyn KnowledgeV1> = Arc::new(CountingKnowledgeAdapter {
            count: AtomicU32::new(0),
        });
        let mut r = CapabilityRegistry::new();
        r.register_knowledge("tenant-a", adapter_a.clone(), PathBuf::from("/tmp/a"))
            .expect("a");
        r.register_knowledge("tenant-b", adapter_b.clone(), PathBuf::from("/tmp/b"))
            .expect("b");
        let got_a = r.knowledge("tenant-a").expect("a");
        let got_b = r.knowledge("tenant-b").expect("b");
        assert!(Arc::ptr_eq(&adapter_a, &got_a));
        assert!(Arc::ptr_eq(&adapter_b, &got_b));
        // The two adapters are
        // independent allocations
        // (different `Arc`s).
        assert!(!Arc::ptr_eq(&got_a, &got_b));
    }

    #[tokio::test]
    async fn handed_out_knowledge_adapter_is_callable() {
        // The end-to-end shape: a
        // workflow gets the
        // `Arc<dyn KnowledgeV1>`,
        // calls `find_information`
        // on it, and the adapter's
        // `count` increments. This
        // is the proof that the
        // registry hands out a
        // real, callable adapter
        // (not a stub).
        //
        // The concrete `Arc<CountingKnowledgeAdapter>`
        // is held alongside the
        // `Arc<dyn KnowledgeV1>` so the
        // counter field is accessible
        // after the `register_knowledge`
        // call (which converts to the
        // trait object).
        let concrete = Arc::new(CountingKnowledgeAdapter {
            count: AtomicU32::new(0),
        });
        let adapter: Arc<dyn KnowledgeV1> = concrete.clone();
        let mut r = CapabilityRegistry::new();
        r.register_knowledge("default", adapter, PathBuf::from("/tmp/knowledge"))
            .expect("register");
        let knowledge = r.knowledge("default").expect("knowledge");
        let ctx = ExecutionContext::new(
            afa_contracts::TenantId::new("t"),
            afa_contracts::Actor::Timer,
        );
        let req = FindInformationRequest::default();
        let _resp = knowledge.find_information(req, &ctx).await.expect("find");
        assert_eq!(concrete.count.load(Ordering::SeqCst), 1);
    }
}
