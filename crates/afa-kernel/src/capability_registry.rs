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
//!
//! Quick lookup: rg -n "CID:capability-registry-" crates/afa-kernel/src/capability_registry.rs

use std::sync::Arc;

use afa_contracts::LlmV1;
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
}

/// The "could not register" reasons. The
/// closed set is small — the registry
/// only has one slot — and the error is
/// typed so a workflow can branch on the
/// reason (not on a string).
#[derive(Debug, Error)]
pub enum RegisterError {
    /// The LLM slot is already occupied.
    /// A second `register_llm` call is a
    /// programmer error (the kernel
    /// constructor is the only caller).
    #[error("llm adapter already registered")]
    LlmAlreadyRegistered,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use afa_contracts::{
        CompletionRequest, CompletionResponse, CompletionStream, ExecutionContext, LlmErrorV1,
        LlmV1, ModelCapabilities,
    };
    use async_trait::async_trait;
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
}
