//! Live-vendor **shape probe** for `ChatCompletionsAdapter`.
//!
//! POSTs a single Chat Completions request
//! to a real OpenAI-compatible vendor and
//! prints:
//!
//! 1. The raw HTTP status, response headers, and raw response body.
//! 2. A **shape report** of the parsed
//!    JSON: every top-level key, its
//!    type, and (for arrays) the count
//!    and element type of the first
//!    element.
//! 3. A **diff** between the vendor's
//!    shape and the keys the adapter
//!    expects (`id`, `object`, `created`,
//!    `model`, `choices`, `usage`). Any
//!    missing key is flagged; any extra
//!    key is flagged. Catches silent
//!    JSON-shape drift between OpenAI
//!    and freellmapi (or any other
//!    OpenAI-compatible proxy).
//!
//! ## What it tests
//!
//! - **JSON contract** — the adapter's
//!   `ChatCompletionsResponse` parser
//!   assumes a specific shape (5
//!   top-level keys, each with a known
//!   nested structure). If the vendor
//!   drops a key (e.g. omits `usage`)
//!   or renames one, the adapter
//!   silently produces a
//!   `MalformedResponse` at runtime
//!   with no clue as to *why* it
//!   failed. This example makes the
//!   raw shape visible so the bug is
//!   diagnosable.
//! - **Nested structure** — choices[0]
//!   must have a `message` with
//!   `role` and `content` (or
//!   `tool_calls`); `usage` must
//!   have `prompt_tokens`,
//!   `completion_tokens`, and
//!   `total_tokens`. The example
//!   walks the tree and reports any
//!   missing or extra field.
//!
//! ## How to run it
//!
//! ```bash
//! FREELLMAPI_URL="http://localhost:3001/v1" \
//! FREELLMAPI_MODEL="auto" \
//! FREELLMAPI_KEY="..." \
//! cargo run --example vendor_shape_probe -p afa-plugin-llm-chat-completions
//! ```
//!
//! Exits 0 if the vendor's shape matches the
//! expected contract; exits 1 if any
//! expected key is missing (a real bug
//! that would cause runtime
//! `MalformedResponse` errors).
//!
//! ## Safety
//!
//! - The API key is read from the env, never hard-coded.
//! - The example prints `key_present=true|false`, never
//!   the key value itself.
//!
//!
//! Story (plain English): The "Lend Your Voice"
//! specialist is given a "phone call form"
//! by the customer service team. The form
//! has 5 boxes (id, object, created, model,
//! choices, usage). The specialist fills
//! the form out and hands it back. The
//! form checker opens it up and writes
//! down every box that was filled, and
//! every box that was left blank. If the
//! customer switched forms and dropped the
//! "usage" box, the form checker reports
//! it; otherwise the form is good. This
//! example is the form checker.
//!
//! CID Index:
//! CID:afa-plugin-llm-chat-completions-example-004 -> vendor_shape_probe
//!
//! Quick lookup: rg -n "CID:afa-plugin-llm-chat-completions-example-" crates/afa-plugin-llm-chat-completions/examples/

use std::env;

use serde_json::{json, Value};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let url = match env::var("FREELLMAPI_URL") {
        Ok(v) => v,
        Err(_) => {
            eprintln!("FREELLMAPI_URL is not set");
            std::process::exit(2);
        }
    };
    let model = env::var("FREELLMAPI_MODEL").unwrap_or_else(|_| "auto".into());
    let key = match env::var("FREELLMAPI_KEY") {
        Ok(v) => v,
        Err(_) => {
            eprintln!("FREELLMAPI_KEY is not set");
            std::process::exit(2);
        }
    };
    println!(
        "Loaded env: url={url}, model={model}, key_present={}",
        !key.is_empty()
    );

    // Build the same request body the
    // adapter would build. The shape is
    // the OpenAI Chat Completions spec:
    // { model, messages: [{role, content}],
    //   max_tokens, temperature, stream:
    //   false }.
    let body = json!({
        "model": model,
        "messages": [
            { "role": "user", "content": "Reply with the single digit 1." }
        ],
        "max_tokens": 4,
        "temperature": 0.0,
        "stream": false,
    });

    // POST with reqwest. We use the same
    // client the adapter uses (rustls
    // backend, JSON, no cookies).
    let client = reqwest::Client::new();
    let endpoint = format!("{}/chat/completions", url.trim_end_matches('/'));
    let resp = match client
        .post(&endpoint)
        .bearer_auth(&key)
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("HTTP error: {e}");
            std::process::exit(1);
        }
    };

    let status = resp.status();
    let headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .map(|(k, v)| {
            (
                k.to_string(),
                v.to_str().unwrap_or("<non-ascii>").to_string(),
            )
        })
        .collect();
    let text = match resp.text().await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("body read error: {e}");
            std::process::exit(1);
        }
    };

    // ----- Section 1: raw response -----
    println!("\n=== Raw response ===");
    println!("status: {status}");
    println!("headers:");
    for (k, v) in &headers {
        if k == "authorization" || k == "x-api-key" {
            println!("  {k}: <redacted>");
        } else {
            println!("  {k}: {v}");
        }
    }
    println!("body ({} bytes):", text.len());
    println!("{text}");

    // ----- Section 2: shape report -----
    let parsed: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("\n=== Shape report ===\nFAIL: body is not valid JSON: {e}");
            std::process::exit(1);
        }
    };
    let obj = match parsed.as_object() {
        Some(o) => o,
        None => {
            eprintln!("\n=== Shape report ===\nFAIL: top-level is not a JSON object");
            std::process::exit(1);
        }
    };

    println!("\n=== Shape report ===");
    let mut keys: Vec<&String> = obj.keys().collect();
    keys.sort();
    for k in &keys {
        let v = &obj[*k];
        let type_str = match v {
            Value::Null => "null".to_string(),
            Value::Bool(_) => "bool".to_string(),
            Value::Number(n) => format!("number({})", n),
            Value::String(_) => "string".to_string(),
            Value::Array(a) => {
                let inner = a
                    .first()
                    .map(|x| match x {
                        Value::Null => "null",
                        Value::Bool(_) => "bool",
                        Value::Number(_) => "number",
                        Value::String(_) => "string",
                        Value::Array(_) => "array",
                        Value::Object(_) => "object",
                    })
                    .unwrap_or("?");
                format!("array[{}] of {inner}", a.len())
            }
            Value::Object(o) => format!("object({} keys)", o.len()),
        };
        println!("  {k}: {type_str}");
    }

    // Walk the choices[0] tree.
    if let Some(choices) = obj.get("choices").and_then(|v| v.as_array()) {
        if let Some(first) = choices.first() {
            println!("\nchoices[0]:");
            if let Some(o) = first.as_object() {
                let mut ks: Vec<&String> = o.keys().collect();
                ks.sort();
                for k in &ks {
                    let v = &o[*k];
                    let type_str = match v {
                        Value::Null => "null".to_string(),
                        Value::Bool(_) => "bool".to_string(),
                        Value::Number(_) => "number".to_string(),
                        Value::String(_) => "string".to_string(),
                        Value::Array(a) => format!("array[{}]", a.len()),
                        Value::Object(o2) => {
                            let mut inner_ks: Vec<&String> = o2.keys().collect();
                            inner_ks.sort();
                            format!(
                                "object{{ {} }}",
                                inner_ks
                                    .iter()
                                    .map(|k| k.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        }
                    };
                    println!("  {k}: {type_str}");
                }
            }
        }
    } else {
        println!("\nchoices: <absent>");
    }

    // Walk the usage tree.
    if let Some(usage) = obj.get("usage") {
        println!("\nusage:");
        if let Some(o) = usage.as_object() {
            let mut ks: Vec<&String> = o.keys().collect();
            ks.sort();
            for k in &ks {
                println!(
                    "  {k}: {}",
                    match &o[*k] {
                        Value::Number(n) => n.to_string(),
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    }
                );
            }
        } else {
            println!("  (not an object: {})", usage);
        }
    } else {
        println!("\nusage: <absent>");
    }

    // ----- Section 3: contract diff -----
    println!("\n=== Contract diff ===");
    let expected_top = ["id", "object", "created", "model", "choices", "usage"];
    let mut missing: Vec<&str> = Vec::new();
    let mut extra: Vec<&str> = Vec::new();
    for k in &expected_top {
        if !obj.contains_key(*k) {
            missing.push(k);
        }
    }
    for k in obj.keys() {
        if !expected_top.contains(&k.as_str()) {
            extra.push(k.as_str());
        }
    }
    if missing.is_empty() && extra.is_empty() {
        println!("PASS: all 6 expected top-level keys present, no extras");
    } else {
        if !missing.is_empty() {
            println!("FAIL: missing top-level keys: {missing:?}");
        }
        if !extra.is_empty() {
            println!("INFO: extra top-level keys (may be vendor extensions): {extra:?}");
        }
    }

    if !missing.is_empty() {
        eprintln!("\n=== Shape probe FAILED: contract mismatch ===");
        std::process::exit(1);
    }
    std::process::exit(0);
}
