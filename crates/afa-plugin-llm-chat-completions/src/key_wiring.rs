//! Code Map: 3-step key wiring pattern (Chat Completions edition)
//! - `UnsealedHolder`: A small struct that holds the
//!   unsealed API key in memory, behind a
//!   `tokio::sync::Mutex`. The 3-step pattern is
//!   identical to the one in `afa-plugin-llm-http`
//!   (cache, retry on 401, zeroize on drop) — the
//!   only difference is the config type
//!   (`ChatCompletionsConfig` here, `ResponsesConfig`
//!   there). See the doc on the
//!   `UnsealedHolder` in
//!   `crates/afa-plugin-llm-http/src/key_wiring.rs`
//!   for the full story.
//!
//! Why duplicated? The holder's `cached` slot is
//! tied to one `config` field (`config.key_ref`).
//! Sharing a single `UnsealedHolder` across the
//! two plugins would force one to depend on the
//! other's config type. The pattern is small
//! (~150 lines); the duplication is a deliberate
//! trade-off. A future "DRY key wiring" pack can
//! extract a `KeyWiring` trait that takes a
//! `SecretRef` (the only field the holder
//! actually needs from the config) and move the
//! struct into `afa-llm`. No behavior change
//! required.
//!
//! CID Index:
//! CID:afa-plugin-llm-chat-completions-key-001 -> UnsealedHolder
//! CID:afa-plugin-llm-chat-completions-key-002 -> get_or_unseal
//! CID:afa-plugin-llm-chat-completions-key-003 -> re_unseal_after_401
//!
//! Quick lookup: rg -n "CID:afa-plugin-llm-chat-completions-key-" crates/afa-plugin-llm-chat-completions/src/key_wiring.rs

use std::sync::Arc;

use afa_contracts::{ExecutionContext, LlmErrorV1, SecurityV1};
use zeroize::Zeroize;

use super::config::ChatCompletionsConfig;

// CID:afa-plugin-llm-chat-completions-key-001 - UnsealedHolder
// Purpose: A small struct that holds the
// unsealed API key in memory, behind a
// `tokio::sync::Mutex`. The pattern has 3
// steps: (1) `get_or_unseal` is called the
// first time the adapter needs the key; it
// calls `security.unseal(key_ref, &ctx)`,
// converts the `UnsealedSecret` to a `String`
// (zeroing the source), and caches the string;
// (2) subsequent calls return the cached
// string; (3) on HTTP 401, the adapter calls
// `re_unseal_after_401` which drops the cached
// string and re-unseals via the security
// engine. On `Drop`, the inner `String` is
// overwritten with zeros (via
// `zeroize::Zeroize`).
// Uses: SecurityV1 (the engine the holder
// delegates to), ExecutionContext,
// UnsealedSecret.
// Used by: ChatCompletionsAdapter (the holder
// is the adapter's field, used in `complete`).
pub struct UnsealedHolder {
    /// The `SecurityV1` engine the holder
    /// delegates to. `Arc` so the holder can
    /// share the engine with the rest of the
    /// kernel (the engine is built once at
    /// startup).
    security: Arc<dyn SecurityV1>,
    /// The static config (carries the
    /// `key_ref`).
    config: ChatCompletionsConfig,
    /// The cached unsealed key. `None`
    /// until `get_or_unseal` is called
    /// for the first time.
    cached: tokio::sync::Mutex<Option<String>>,
}

impl UnsealedHolder {
    /// Build a new `UnsealedHolder`. The
    /// `cached` slot starts empty; the
    /// first call to `get_or_unseal`
    /// performs the unseal.
    pub fn new(security: Arc<dyn SecurityV1>, config: ChatCompletionsConfig) -> Self {
        Self {
            security,
            config,
            cached: tokio::sync::Mutex::new(None),
        }
    }

    // CID:afa-plugin-llm-chat-completions-key-002 - get_or_unseal
    // Purpose: Step 1 of the 3-step pattern.
    // Returns the cached key if it exists;
    // otherwise unseals via the security
    // engine, caches, and returns. The
    // returned `String` is a normal owned
    // `String` (the security engine's
    // `UnsealedSecret` is consumed and
    // zeroed on the way out, so the
    // secret never escapes its scope in
    // a form the compiler can prove is
    // zeroized).
    // Uses: SecurityV1, ExecutionContext,
    // SecretRef.
    // Used by: ChatCompletionsAdapter (the
    // first line of every request).
    pub async fn get_or_unseal(&self, ctx: &ExecutionContext) -> Result<String, LlmErrorV1> {
        // Fast path: the cache is warm.
        // We take a brief read-lock via
        // `lock().await` and check `Some`.
        // If so, we clone the string out
        // (the lock is released on drop).
        {
            let guard = self.cached.lock().await;
            if let Some(s) = guard.as_ref() {
                return Ok(s.clone());
            }
        }
        // Slow path: the cache is cold.
        // We lock again, double-check
        // (another caller may have raced
        // us to unseal), then unseal via
        // the security engine's `unseal`
        // method. The trait takes a
        // `&SecretRef` (the pre-sealed
        // receipt) — not a name string.
        // The operator pre-seals the API
        // key at startup, captures the
        // `SecretRef`, and passes it via
        // `ChatCompletionsConfig`.
        let mut guard = self.cached.lock().await;
        if guard.is_none() {
            let secret = self
                .security
                .unseal(&self.config.key_ref, ctx)
                .await
                .map_err(|e| LlmErrorV1::AuthenticationFailed {
                    reason: format!("unseal failed: {e:?}"),
                })?;
            // The `UnsealedSecret` is a
            // `Zeroizing<Vec<u8>>` that
            // derefs to `&[u8]`. We copy
            // the bytes into a local
            // `Vec`, build a `String` from
            // them, and zeroize the local
            // `Vec` immediately so the
            // plaintext does not linger in
            // a `Debug`-able form. The
            // `secret` handle's own drop
            // (at end of `if guard.is_none()`)
            // zeros its own buffer.
            let mut bytes: Vec<u8> = secret.to_vec();
            let s = match String::from_utf8(bytes.clone()) {
                Ok(s) => s,
                Err(_) => {
                    bytes.zeroize();
                    return Err(LlmErrorV1::AuthenticationFailed {
                        reason: "unsealed key is not valid UTF-8".into(),
                    });
                }
            };
            bytes.zeroize();
            *guard = Some(s);
        }
        Ok(guard.as_ref().expect("just inserted").clone())
    }

    // CID:afa-plugin-llm-chat-completions-key-003 - re_unseal_after_401
    // Purpose: Step 3 of the 3-step pattern.
    // Drops the cached key and re-unseals.
    // Called when the vendor returns HTTP
    // 401 (the bearer token is invalid —
    // almost always because the key was
    // rotated by the operator and the
    // engine's storage now holds a new
    // sealed value).
    // Uses: SecurityV1, ExecutionContext.
    // Used by: ChatCompletionsAdapter (in
    // the 401 retry branch of `complete`).
    pub async fn re_unseal_after_401(&self, ctx: &ExecutionContext) -> Result<String, LlmErrorV1> {
        // Step 1: drop the cached
        // string. We zeroize it first
        // (the `Drop` of `String` does
        // not zero its memory — the
        // allocator may reuse the
        // bytes), then `take()` it out
        // of the slot.
        {
            let mut guard = self.cached.lock().await;
            if let Some(mut s) = guard.take() {
                s.zeroize();
            }
        }
        // Step 2: re-unseal. The
        // next `get_or_unseal` call
        // will see `None` and go
        // through the slow path.
        self.get_or_unseal(ctx).await
    }

    /// Expose the underlying
    /// `SecurityV1` engine as an
    /// `Arc` so callers (the
    /// streaming bg task) can use
    /// it without going through
    /// the holder's cache. The
    /// streaming path is a single
    /// round-trip so caching the
    /// key in the holder is
    /// pointless; a fresh
    /// `unseal()` keeps the
    /// plaintext in a different
    /// zeroize-on-drop scope.
    pub fn share_security_arc(&self) -> Arc<dyn SecurityV1> {
        self.security.clone()
    }
}

impl Drop for UnsealedHolder {
    /// Wipe the cached key (if any) when
    /// the holder is dropped. The kernel
    /// does this when the adapter is
    /// dropped (at process shutdown, or
    /// when the adapter is replaced by a
    /// config reload).
    fn drop(&mut self) {
        // `try_lock` because we are in a
        // sync context (no `.await` in
        // `Drop`); if the lock is held
        // by a pending request, the
        // request will return its
        // cloned `String` and the inner
        // `Option<String>` will be
        // dropped at the end of the
        // request. This is the
        // best-effort part of the
        // zeroize-on-drop story.
        if let Ok(mut guard) = self.cached.try_lock() {
            if let Some(mut s) = guard.take() {
                s.zeroize();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ChatCompletionsConfig;
    use afa_contracts::UnsealedSecret;
    use afa_contracts::{Actor, SecretRef, SecurityErrorV1, TenantId};
    use async_trait::async_trait;

    /// A fake `SecurityV1` that returns a
    /// pre-set plaintext the first time
    /// and a *different* plaintext on
    /// every later call. Lets the test
    /// assert the holder's cache +
    /// re-unseal behavior.
    struct FakeSecurity {
        new_calls: std::sync::Mutex<u32>,
    }

    #[async_trait]
    impl SecurityV1 for FakeSecurity {
        async fn seal(&self, _plaintext: &[u8], _name: &str) -> Result<SecretRef, SecurityErrorV1> {
            unimplemented!()
        }
        async fn unseal(
            &self,
            _name: &SecretRef,
            _ctx: &afa_contracts::ExecutionContext,
        ) -> Result<UnsealedSecret, SecurityErrorV1> {
            let mut n = self.new_calls.lock().unwrap();
            *n += 1;
            // First call: key v1.
            // Second call: key v2
            // (the "rotated" value).
            let bytes = if *n == 1 {
                b"sk-v1".to_vec()
            } else {
                b"sk-v2".to_vec()
            };
            Ok(UnsealedSecret::new(bytes))
        }
        async fn rotate(
            &self,
            _secret_ref: &SecretRef,
            _new_plaintext: &[u8],
            _ctx: &afa_contracts::ExecutionContext,
        ) -> Result<SecretRef, SecurityErrorV1> {
            unimplemented!()
        }
    }

    fn ctx() -> ExecutionContext {
        ExecutionContext::new(TenantId::new("test"), Actor::Timer)
    }

    fn key_ref() -> SecretRef {
        SecretRef {
            name: "freellmapi-key".into(),
            version: 1,
        }
    }

    #[tokio::test]
    async fn get_or_unseal_caches_on_second_call() {
        // The first `get_or_unseal` call
        // triggers a `security.unseal`
        // and caches the result; the
        // second call returns the cached
        // value without calling
        // `security.unseal` a second
        // time. The `FakeSecurity`
        // records its call count, and we
        // assert it is exactly 1.
        let security = Arc::new(FakeSecurity {
            new_calls: std::sync::Mutex::new(0),
        });
        let holder = UnsealedHolder::new(
            security.clone(),
            ChatCompletionsConfig::gpt_4o_mini(key_ref()),
        );
        let s1 = holder.get_or_unseal(&ctx()).await.expect("v1");
        assert_eq!(s1, "sk-v1");
        let s2 = holder.get_or_unseal(&ctx()).await.expect("cached");
        assert_eq!(s2, "sk-v1");
        // The second call must NOT have
        // triggered a second `unseal`
        // (the cache is warm). The
        // fake's counter is exactly 1.
        assert_eq!(*security.new_calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn re_unseal_after_401_picks_up_the_rotated_key() {
        // After the vendor returns 401,
        // the adapter calls
        // `re_unseal_after_401`. The
        // holder drops the cached key
        // and re-unseals. The fake's
        // second call returns the new
        // key (`"sk-v2"`). The adapter
        // uses the new key on retry.
        let security = Arc::new(FakeSecurity {
            new_calls: std::sync::Mutex::new(0),
        });
        let holder = UnsealedHolder::new(
            security.clone(),
            ChatCompletionsConfig::gpt_4o_mini(key_ref()),
        );
        let s1 = holder.get_or_unseal(&ctx()).await.expect("v1");
        assert_eq!(s1, "sk-v1");
        let s2 = holder.re_unseal_after_401(&ctx()).await.expect("v2");
        assert_eq!(s2, "sk-v2");
        // The fake's counter is 2
        // (first call + re-unseal).
        assert_eq!(*security.new_calls.lock().unwrap(), 2);
    }
}
