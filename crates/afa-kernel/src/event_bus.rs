//! Code Map: In-process pub/sub event bus
//! - `EventBus`: The pub/sub broker. Publishers call `publish`,
//!   subscribers call `subscribe` to get a `Subscription<T>`.
//! - `Subscription<T>`: The receiving end of a single subscription.
//!   `recv()` yields the next `(Arc<T>, ExecutionContext)` pair.
//! - `EventBusHandle`: A thin wrapper around an `Arc<EventBus>` that
//!   exposes only `publish` (no `subscribe`). Steps receive a
//!   handle to publish their own events without being able to peek
//!   at the bus's other subscribers.
//! - `DEFAULT_SUBSCRIPTION_CAPACITY`: The recommended default
//!   capacity for a subscription's bounded channel. Callers can
//!   override this if they expect bursty traffic.
//! - `SendError` (private): The outcome of a non-blocking send
//!   attempt: `Ok(())` if the item was queued, `Full` if the
//!   channel is at capacity (caller should retry with an async
//!   send), or `Closed` if the receiver was dropped (caller
//!   should remove the subscription from the registry).
//!
//! Story (plain English): Imagine a public bulletin board in a
//! community center. Anyone can pin a notice (publish), and
//! anyone who has signed up for a particular kind of notice
//! (subscribed to an event type) gets a copy. The board itself
//! does not care what is written on the notices — it is just
//! paper. If a pile of notices for one person gets too tall
//! (the channel is full), the board hands the new notice to a
//! helper who will put it on the pile as soon as there is room
//! (the async fallback task). If a person has moved away (the
//! receiver was dropped), the board stops trying to give them
//! notices (lazy removal). The board's job is just to route; it
//! does not read the notices.
//!
//! CID Index:
//! CID:event-bus-001 -> EventBus
//! CID:event-bus-002 -> `Subscription<T>`
//! CID:event-bus-003 -> EventBusHandle
//! CID:event-bus-004 -> DEFAULT_SUBSCRIPTION_CAPACITY
//! CID:event-bus-005 -> SendError (private)
//! CID:event-bus-006 -> ErasedSender (private trait)
//! CID:event-bus-007 -> `TypedSender<T>` (private)
//!
//! Quick lookup: rg -n "CID:event-bus-" crates/afa-kernel/src/event_bus.rs

use afa_contracts::{AfaEvent, ExecutionContext};
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// The default capacity for a new subscription's bounded channel.
/// Callers that expect bursty traffic should pick a larger
/// capacity when they call `subscribe`; this constant is a
/// reasonable starting point for low- to moderate-frequency event
/// types.
pub const DEFAULT_SUBSCRIPTION_CAPACITY: usize = 1024;

/// The full type of the bus's internal registry. Factored out
/// so the `Arc<RwLock<HashMap<...>>>` shape only has to be
/// spelled once (and so clippy's `type_complexity` lint is
/// satisfied).
type Registry = Arc<RwLock<HashMap<TypeId, Vec<Arc<dyn ErasedSender>>>>>;

// CID:event-bus-001 - EventBus
// Purpose: The in-process pub/sub broker. Holds a registry of
// typed senders keyed by event-type `TypeId`. Publishers call
// `publish` to fan out to all registered subscribers;
// subscribers call `subscribe` to register a new typed
// channel. Cloning is intentionally not implemented — the
// intended sharing path is `Arc<EventBus>` (e.g. inside
// `Kernel`).
// Uses: tokio::sync::mpsc (bounded channels), std::sync::RwLock
// (the registry mutex, held only across sync operations).
// Used by: `Kernel` (owns one), `Runtime` (calls `publish` to
// emit `EventReceived`), `Scheduler` (hands a step an
// `EventBusHandle` that wraps an `Arc<EventBus>`).
pub struct EventBus {
    registry: Registry,
}

impl EventBus {
    /// Build a fresh, empty `EventBus`.
    pub fn new() -> Self {
        Self {
            registry: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register interest in events of type `T` and return a
    /// `Subscription<T>` that yields each published instance.
    ///
    /// `capacity` is the bound on the subscription's internal
    /// `mpsc` channel. When the channel is at capacity and a new
    /// event publishes, the publish call does not block — the
    /// event is delivered asynchronously by a fallback task (see
    /// `EventBus::publish`).
    pub fn subscribe<T: AfaEvent>(&self, capacity: usize) -> Subscription<T> {
        let (tx, rx) = mpsc::channel(capacity);
        let typed = Arc::new(TypedSender::<T> {
            sender: tx,
            _phantom: PhantomData,
        });
        let erased: Arc<dyn ErasedSender> = typed;
        self.registry
            .write()
            .expect("EventBus registry poisoned")
            .entry(TypeId::of::<T>())
            .or_default()
            .push(erased);
        Subscription { receiver: rx }
    }

    /// Fan out an event of type `T` to every currently
    /// registered subscriber.
    ///
    /// Behavior:
    /// - If a subscriber's channel has room, the event is queued
    ///   immediately and the call returns control to the caller.
    /// - If a subscriber's channel is full, a `tokio::spawn`-ed
    ///   task performs the blocking `send().await` so the slow
    ///   subscriber cannot stall the publisher. A `tracing::warn!`
    ///   is emitted to flag the backpressure.
    /// - If a subscriber's receiver has been dropped (the
    ///   `Subscription<T>` was dropped), the entry is logged at
    ///   `debug` level and removed from the registry after the
    ///   fan-out loop.
    /// - If no subscriber is registered for `T`, the call is a
    ///   no-op (an event with zero reactors is an ordinary
    ///   condition, not an error).
    pub async fn publish<T: AfaEvent>(&self, event: T, ctx: ExecutionContext) {
        let event = Arc::new(event);
        let type_name = std::any::type_name::<T>();

        // Snapshot the list of senders under the read lock, then
        // release the lock before any work that could be slow.
        let senders: Vec<Arc<dyn ErasedSender>> = {
            let registry = self.registry.read().expect("EventBus registry poisoned");
            registry
                .get(&TypeId::of::<T>())
                .cloned()
                .unwrap_or_default()
        };

        if senders.is_empty() {
            debug!(event_type = type_name, "publish with zero subscribers");
            return;
        }

        let mut closed_indices: Vec<usize> = Vec::new();
        for (idx, sender) in senders.iter().enumerate() {
            let erased: Arc<dyn Any + Send + Sync> = event.clone();
            match sender.try_send_erased(erased, ctx.clone()) {
                Ok(()) => {}
                Err(SendError::Full) => {
                    let erased_fallback: Arc<dyn Any + Send + Sync> = event.clone();
                    sender.spawn_full_send(erased_fallback, ctx.clone(), type_name);
                }
                Err(SendError::Closed) => {
                    closed_indices.push(idx);
                }
            }
        }

        if !closed_indices.is_empty() {
            let mut registry = self.registry.write().expect("EventBus registry poisoned");
            if let Some(list) = registry.get_mut(&TypeId::of::<T>()) {
                // Remove in reverse so indices stay valid.
                for &idx in closed_indices.iter().rev() {
                    list.remove(idx);
                }
            }
        }
    }

    /// Build an `EventBusHandle` that publishes through this bus.
    /// Used by `Scheduler` to hand a step a "publish-only" view of
    /// the bus.
    #[allow(dead_code)] // Used by the Scheduler in Phase 2.
    pub(crate) fn handle(&self) -> EventBusHandle {
        EventBusHandle {
            inner: Arc::new(EventBusCore {
                registry: Arc::clone(&self.registry),
            }),
        }
    }

    /// Test-only: report the number of registered senders for
    /// the given event type. Used by the closed-subscription
    /// test to confirm the dead entry was removed.
    #[cfg(test)]
    pub(crate) fn subscriber_count<T: AfaEvent>(&self) -> usize {
        self.registry
            .read()
            .expect("EventBus registry poisoned")
            .get(&TypeId::of::<T>())
            .map(|v| v.len())
            .unwrap_or(0)
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for EventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBus").finish_non_exhaustive()
    }
}

// CID:event-bus-002 - Subscription<T>
// Purpose: The receiving end of a single subscription. Each
// `subscribe` call returns one of these, wrapping a typed
// `mpsc::Receiver`. The `recv` method yields the next
// `(Arc<T>, ExecutionContext)` pair delivered by the bus.
// Uses: tokio::sync::mpsc::Receiver.
// Used by: any code that wants to react to a published event
// type — the kernel's own tests, future engines, future
// plugins, future observability code.
pub struct Subscription<T: AfaEvent> {
    receiver: mpsc::Receiver<(Arc<T>, ExecutionContext)>,
}

impl<T: AfaEvent> Subscription<T> {
    /// Wait for the next event delivered to this subscription.
    /// Returns `None` when the bus has been dropped and no more
    /// events will ever arrive on this subscription.
    pub async fn recv(&mut self) -> Option<(Arc<T>, ExecutionContext)> {
        self.receiver.recv().await
    }
}

impl<T: AfaEvent> std::fmt::Debug for Subscription<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscription")
            .field("event_type", &std::any::type_name::<T>())
            .finish_non_exhaustive()
    }
}

// CID:event-bus-003 - EventBusHandle
// Purpose: A thin "publish-only" view of an `EventBus`. Steps
// receive a handle so they can publish their own events
// without being able to subscribe or otherwise inspect the
// bus's other subscribers. This is the same pattern the
// `Kernel` uses to give the `Scheduler` and `Runtime` their
// own views.
// Uses: a private wrapper `EventBusCore` that holds the
// shared registry `Arc` and exposes a `publish` method.
// Used by: `Scheduler` (hands one to each registered step).
pub struct EventBusHandle {
    inner: Arc<EventBusCore>,
}

impl EventBusHandle {
    /// Publish an event through this handle. See
    /// `EventBus::publish` for the full fan-out and
    /// backpressure behavior — this method is a thin
    /// pass-through to it.
    pub async fn publish<T: AfaEvent>(&self, event: T, ctx: ExecutionContext) {
        self.inner.publish(event, ctx).await;
    }
}

impl Clone for EventBusHandle {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl std::fmt::Debug for EventBusHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBusHandle").finish_non_exhaustive()
    }
}

/// Private inner view used by `EventBusHandle`. Holds the
/// shared registry `Arc` directly (without the `EventBus`
/// struct's other fields, of which there are none — this
/// exists for symmetry with the future design and so that
/// `EventBusHandle` does not retain a full `EventBus` worth
/// of dependencies in its type signature).
struct EventBusCore {
    registry: Registry,
}

impl EventBusCore {
    async fn publish<T: AfaEvent>(&self, event: T, ctx: ExecutionContext) {
        let event = Arc::new(event);
        let type_name = std::any::type_name::<T>();

        let senders: Vec<Arc<dyn ErasedSender>> = {
            let registry = self.registry.read().expect("EventBus registry poisoned");
            registry
                .get(&TypeId::of::<T>())
                .cloned()
                .unwrap_or_default()
        };

        if senders.is_empty() {
            debug!(event_type = type_name, "publish with zero subscribers");
            return;
        }

        let mut closed_indices: Vec<usize> = Vec::new();
        for (idx, sender) in senders.iter().enumerate() {
            let erased: Arc<dyn Any + Send + Sync> = event.clone();
            match sender.try_send_erased(erased, ctx.clone()) {
                Ok(()) => {}
                Err(SendError::Full) => {
                    let erased_fallback: Arc<dyn Any + Send + Sync> = event.clone();
                    sender.spawn_full_send(erased_fallback, ctx.clone(), type_name);
                }
                Err(SendError::Closed) => {
                    closed_indices.push(idx);
                }
            }
        }

        if !closed_indices.is_empty() {
            let mut registry = self.registry.write().expect("EventBus registry poisoned");
            if let Some(list) = registry.get_mut(&TypeId::of::<T>()) {
                for &idx in closed_indices.iter().rev() {
                    list.remove(idx);
                }
            }
        }
    }
}

// CID:event-bus-005 - SendError (private)
// Purpose: The outcome of a non-blocking `try_send_erased`
// attempt. `Ok(())` means the event was queued. `Full` means
// the channel is at capacity and the caller should retry with
// an async send. `Closed` means the receiver was dropped and
// the caller should remove the entry from the registry.
// Uses: nothing external — it is a local error vocabulary.
// Used by: `ErasedSender::try_send_erased`, the fan-out loop
// in `EventBus::publish` and `EventBusCore::publish`.
enum SendError {
    Full,
    Closed,
}

// CID:event-bus-006 - ErasedSender (private trait)
// Purpose: A type-erased view of a `mpsc::Sender<(Arc<T>,
// ExecutionContext)>` so the bus's registry can hold senders
// for many different concrete `T`s behind one map. The trait
// is object-safe (no generics, no `Self` in return position)
// so it can be stored as `Arc<dyn ErasedSender>`.
// Used by: the registry; the fan-out loops in
// `EventBus::publish` and `EventBusCore::publish`.
trait ErasedSender: Send + Sync {
    /// Non-blocking send. On `Ok(())`, the event is queued
    /// and the caller is done with this sender. On `Full`,
    /// the caller should clone the event and call
    /// `spawn_full_send` for asynchronous delivery. On
    /// `Closed`, the caller should remove this sender from
    /// the registry.
    fn try_send_erased(
        &self,
        event: Arc<dyn Any + Send + Sync>,
        ctx: ExecutionContext,
    ) -> Result<(), SendError>;

    /// Spawn a task to perform the blocking `send().await` for
    /// the `Full` case. The task logs a `warn!` on success
    /// (backpressure was applied) and a `debug!` on closure
    /// (the channel was closed between `try_send` and
    /// `send`).
    fn spawn_full_send(
        &self,
        event: Arc<dyn Any + Send + Sync>,
        ctx: ExecutionContext,
        event_type_name: &'static str,
    );
}

// CID:event-bus-007 - TypedSender<T> (private)
// Purpose: The concrete `ErasedSender` implementation for a
// specific event type `T`. Wraps an `mpsc::Sender<(Arc<T>,
// ExecutionContext)>` and downcasts the incoming
// `Arc<dyn Any + Send + Sync>` back to `Arc<T>` on each
// send. The downcast is guaranteed to succeed because the
// `TypedSender<T>` is only ever installed in the registry
// under `TypeId::of::<T>()` — see `EventBus::subscribe`.
// Uses: `PhantomData<fn() -> T>` so the type is
// `Send + Sync` regardless of `T`'s auto-traits (this is
// the standard "sendable phantom" pattern).
// Used by: `EventBus::subscribe` (one created per
// subscription, then stored as `Arc<dyn ErasedSender>`).
struct TypedSender<T: AfaEvent> {
    sender: mpsc::Sender<(Arc<T>, ExecutionContext)>,
    _phantom: PhantomData<fn() -> T>,
}

impl<T: AfaEvent> ErasedSender for TypedSender<T> {
    fn try_send_erased(
        &self,
        event: Arc<dyn Any + Send + Sync>,
        ctx: ExecutionContext,
    ) -> Result<(), SendError> {
        // Safety: this sender is installed in the registry
        // under `TypeId::of::<T>()`, and the only caller of
        // `try_send_erased` is the fan-out loop in `publish`,
        // which is itself generic over the same `T` and looks
        // up senders by the same `TypeId`. The downcast
        // therefore cannot fail.
        let event: Arc<T> = event
            .downcast::<T>()
            .expect("TypedSender<T> downcast: TypeId mismatch");
        match self.sender.try_send((event, ctx)) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => Err(SendError::Full),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(SendError::Closed),
        }
    }

    fn spawn_full_send(
        &self,
        event: Arc<dyn Any + Send + Sync>,
        ctx: ExecutionContext,
        event_type_name: &'static str,
    ) {
        let event: Arc<T> = event
            .downcast::<T>()
            .expect("TypedSender<T> downcast: TypeId mismatch");
        let sender = self.sender.clone();
        tokio::spawn(async move {
            match sender.send((event, ctx)).await {
                Ok(()) => {
                    warn!(
                        event_type = event_type_name,
                        "subscriber channel was full; delivered asynchronously (backpressure)"
                    );
                }
                Err(_) => {
                    // The channel closed between `try_send`
                    // and `send`. This is rare but possible if
                    // the receiver was dropped in the same
                    // window; the lazy-removal sweep on the
                    // next publish will clean it up.
                    debug!(
                        event_type = event_type_name,
                        "subscriber channel closed during async fallback send"
                    );
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use afa_contracts::{Actor, TenantId};
    use serde::{Deserialize, Serialize};

    // A simple illustrative event used by the tests in this
    // module. Lives in `tests/` of the test file's own scope
    // (not in `src/`) per the IMPL's planning principle: only
    // the events named explicitly by the architecture docs
    // (`EventReceived`, `WorkflowStepFailed`) ship in `src/`.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct TestEvent {
        payload: String,
    }

    impl AfaEvent for TestEvent {}

    fn fresh_ctx() -> ExecutionContext {
        ExecutionContext::new(TenantId::new("test-tenant"), Actor::Timer)
    }

    #[tokio::test]
    async fn publish_then_recv_yields_event_and_context() {
        let bus = EventBus::new();
        let mut sub = bus.subscribe::<TestEvent>(16);

        let ctx = fresh_ctx();
        let expected_cid = ctx.correlation_id;
        bus.publish(
            TestEvent {
                payload: "hello".into(),
            },
            ctx,
        )
        .await;

        let (received, recv_ctx) = sub.recv().await.expect("event should arrive");
        assert_eq!(received.payload, "hello");
        assert_eq!(recv_ctx.correlation_id, expected_cid);
    }

    #[tokio::test]
    async fn two_subscribers_both_receive_every_published_instance() {
        let bus = EventBus::new();
        let mut sub_a = bus.subscribe::<TestEvent>(16);
        let mut sub_b = bus.subscribe::<TestEvent>(16);

        bus.publish(
            TestEvent {
                payload: "first".into(),
            },
            fresh_ctx(),
        )
        .await;
        bus.publish(
            TestEvent {
                payload: "second".into(),
            },
            fresh_ctx(),
        )
        .await;

        let (a1, _) = sub_a.recv().await.expect("a/1");
        let (a2, _) = sub_a.recv().await.expect("a/2");
        let (b1, _) = sub_b.recv().await.expect("b/1");
        let (b2, _) = sub_b.recv().await.expect("b/2");

        assert_eq!(a1.payload, "first");
        assert_eq!(a2.payload, "second");
        assert_eq!(b1.payload, "first");
        assert_eq!(b2.payload, "second");
    }

    #[tokio::test]
    async fn full_subscriber_queue_uses_async_fallback_and_does_not_block_publisher() {
        let bus = EventBus::new();
        // Capacity 1; we publish 2 events without reading.
        let mut sub = bus.subscribe::<TestEvent>(1);

        bus.publish(
            TestEvent {
                payload: "first".into(),
            },
            fresh_ctx(),
        )
        .await;
        // The second publish must not block, even though the
        // channel is full — the fallback task will deliver it.
        bus.publish(
            TestEvent {
                payload: "second".into(),
            },
            fresh_ctx(),
        )
        .await;

        // Drain the subscription; both events should arrive.
        let (e1, _) = sub.recv().await.expect("first event");
        let (e2, _) = sub
            .recv()
            .await
            .expect("second event (delivered via fallback)");
        assert_eq!(e1.payload, "first");
        assert_eq!(e2.payload, "second");
    }

    #[tokio::test]
    async fn dropped_subscription_is_removed_on_next_publish() {
        let bus = EventBus::new();

        {
            let _sub = bus.subscribe::<TestEvent>(4);
            assert_eq!(bus.subscriber_count::<TestEvent>(), 1);
        }
        // `_sub` is now dropped; the next publish should detect
        // the closed channel and remove the entry from the
        // registry.
        bus.publish(
            TestEvent {
                payload: "to nobody".into(),
            },
            fresh_ctx(),
        )
        .await;
        assert_eq!(bus.subscriber_count::<TestEvent>(), 0);
    }

    #[tokio::test]
    async fn publishing_with_zero_subscribers_does_not_error_or_panic() {
        let bus = EventBus::new();
        // No subscription registered. Publish must succeed
        // (it's a no-op fan-out).
        bus.publish(
            TestEvent {
                payload: "into the void".into(),
            },
            fresh_ctx(),
        )
        .await;
        // Reaching here without panic is the assertion.
        assert_eq!(bus.subscriber_count::<TestEvent>(), 0);
    }

    #[tokio::test]
    async fn event_bus_handle_publishes_through_shared_bus() {
        // The handle must be a publish-only view: building one
        // and publishing through it must deliver to a
        // subscription registered against the same bus.
        let bus = EventBus::new();
        let mut sub = bus.subscribe::<TestEvent>(16);
        let handle = bus.handle();

        handle
            .publish(
                TestEvent {
                    payload: "via handle".into(),
                },
                fresh_ctx(),
            )
            .await;

        let (received, _) = sub.recv().await.expect("event via handle");
        assert_eq!(received.payload, "via handle");
    }
}
