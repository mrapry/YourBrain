//! Hook handler (ADR-9): reads a JSON payload from **stdin** and records it.
//!
//! Real IDE hooks pipe a JSON object on stdin (not CLI args). This handler is
//! intentionally fast and forgiving: it never fails the caller's operation, so
//! a malformed payload or missing session is swallowed with a non-zero-free exit.
//!
//! In this build the handler writes directly (in-process). The daemon variant
//! (ADR-1/ADR-2) would instead forward the event over IPC; the recording logic
//! is identical and lives on [`yb_core::Brain`].

use std::io::Read;

use anyhow::Result;
use serde_json::Value;

use crate::context;

/// Handle a hook event of the given kind, reading its JSON payload from stdin.
pub fn run(event: &str) -> Result<()> {
    let mut buf = String::new();
    // Hooks may send an empty body for some events; tolerate that.
    let _ = std::io::stdin().read_to_string(&mut buf);
    let payload: Value = serde_json::from_str(buf.trim()).unwrap_or(Value::Null);

    let brain = match context::open_brain() {
        Ok(b) => b,
        // Never break the IDE if the brain can't open.
        Err(e) => {
            eprintln!("yb hook: brain unavailable: {e}");
            return Ok(());
        }
    };

    let get = |k: &str| payload.get(k).and_then(|v| v.as_str()).map(String::from);
    let ide = get("ide").unwrap_or_else(|| "unknown".into());

    match event {
        "session_start" => {
            // Prefer the IDE-provided session id (stable across this session's
            // later hooks); only mint our own when the IDE doesn't supply one.
            let id = match get("session_id") {
                Some(sid) => {
                    brain.ensure_session(&sid, &ide, get("cwd"), get("room"))?;
                    sid
                }
                None => brain.start_session(&ide, get("cwd"), get("room"))?,
            };
            // Emit the session id so the IDE can thread it through later hooks.
            println!("{{\"session_id\":\"{id}\"}}");
        }
        "prompt_submit" => {
            if let (Some(session), Some(content)) =
                (get("session_id"), get("content").or_else(|| get("prompt")))
            {
                brain.ensure_session(&session, &ide, get("cwd"), get("room"))?;
                brain.add_observation(&session, "prompt", &content)?;
            }
        }
        "tool_use" => {
            if let Some(session) = get("session_id") {
                brain.ensure_session(&session, &ide, get("cwd"), get("room"))?;
                let tool = get("tool").unwrap_or_default();
                let result = get("result").unwrap_or_default();
                let summary = format!("{tool}: {result}");
                brain.add_observation(&session, "tool_use", &summary)?;
            }
        }
        "ai_response" => {
            if let (Some(session), Some(content)) = (get("session_id"), get("content")) {
                brain.ensure_session(&session, &ide, get("cwd"), get("room"))?;
                brain.add_observation(&session, "response", &content)?;
            }
        }
        "session_end" => {
            if let Some(session) = get("session_id") {
                brain.end_session(&session)?;
            }
        }
        other => {
            eprintln!("yb hook: unknown event `{other}` (ignored)");
        }
    }
    Ok(())
}
