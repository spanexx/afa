//! Code Map: Event convention
//! - `AfaEvent`: A "yes, I'm an event" badge. Anything that wants to
//!   be put on the event bus wears this badge.
//! - `ExampleLessonCreated`: A sample event used by the
//!   conformance tests to prove the badge works. Not a real
//!   production event — the first real one comes with the
//!   `kernel-core` pack.
//!
//! Story (plain English): Imagine a town bulletin board. People pin
//! notices to it; others come by and read the board to find out
//! what happened. The badge rule here is: every notice must (a) be
//! something you can write down on paper, (b) be something you can
//! read back from paper later, and (c) be safe to read at any time.
//! That badge is `AfaEvent`. Because every notice has to be
//! readable from paper, the bulletin board doesn't accept vague
//! "a notice of some kind" notices — you can't pin a `Box<dyn
//! Notice>` because paper doesn't know what shape the original
//! notice was. This is the rule the compile-fail check below
//! guards.
//!
//! CID Index:
//! CID:events-001 -> AfaEvent
//! CID:events-002 -> ExampleLessonCreated
//!
//! Quick lookup: rg -n "CID:events-" crates/afa-contracts/src/events.rs

use crate::ids::CorrelationId;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

// CID:events-001 - AfaEvent
// Purpose: A "I'm an event" badge. Anything that wants to ride the
// event bus must wear it. The three rules (can be written to JSON,
// can be read back from JSON, can be shared across threads and
// live as long as any handler) are baked in as trait bounds.
// Uses: serde (the JSON format the kernel uses for transport).
// Used by: the event bus (to know what types may be published),
// and any engine or plugin that wants to publish events.
pub trait AfaEvent: Serialize + DeserializeOwned + Send + Sync + 'static {}

// CID:events-002 - ExampleLessonCreated
// Purpose: A sample event used only by the conformance tests, to
// prove the `AfaEvent` badge works end-to-end (it can be
// serialized, deserialized, and tagged with a tracking number).
// Marked `#[non_exhaustive]` so future fields can be added
// without breaking anyone who already uses it.
// Uses: CorrelationId (every event carries its request's tracking
// number).
// Used by: the conformance test in `afa-contract-testing`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ExampleLessonCreated {
    pub correlation_id: CorrelationId,
    pub note: String,
}

impl AfaEvent for ExampleLessonCreated {}

/// Compile-fail check: `Box<dyn AfaEvent>` is rejected because
/// `DeserializeOwned` is not object-safe. This `compile_fail` doctest
/// is part of `cargo test` and acts as the regression-proof assertion
/// for FLOW Flow 6 Edge case B.
///
/// ```compile_fail
/// use afa_contracts::events::AfaEvent;
/// fn _reject_boxed(_b: Box<dyn AfaEvent>) {}
/// ```
#[allow(dead_code)]
fn _afaeevent_dyn_rejection_marker() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_lesson_created_round_trips_through_json() {
        let original = ExampleLessonCreated {
            correlation_id: CorrelationId::new(),
            note: "a lesson was learned".to_string(),
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let decoded: ExampleLessonCreated = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, decoded);
    }
}
