//! End-to-end integration tests over the public `Brain` API, exercising the
//! on-disk store, vector index persistence, conflict flow, and import/export.

use yb_core::brain::{RememberOptions, RememberOutcome};
use yb_core::conflict::ResolutionAction;
use yb_core::memory::MemoryState;
use yb_core::search::DetailLevel;
use yb_core::{Brain, Config};

fn brain_at(dir: &std::path::Path) -> Brain {
    Brain::open(dir, Config::default()).unwrap()
}

#[test]
fn full_flow_persists_across_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let id = {
        let mut b = brain_at(tmp.path());
        let out = b
            .remember(
                "Backend API uses Rust with the Axum framework",
                RememberOptions::default(),
            )
            .unwrap();
        b.save().unwrap();
        match out {
            RememberOutcome::Stored { id } => id,
            other => panic!("expected stored, got {other:?}"),
        }
    };

    // Reopen: memory + embedding must survive, recall must find it.
    let b = brain_at(tmp.path());
    let got = b.get(&id).unwrap().expect("memory should persist");
    assert_eq!(got.state, MemoryState::Active);

    let res = b
        .recall(
            "what web framework does the backend use",
            5,
            DetailLevel::Summary,
            200,
            None,
        )
        .unwrap();
    assert!(
        res.output.ids.contains(&id),
        "recall should find the reopened memory"
    );
}

#[test]
fn conflict_replace_archives_existing() {
    let tmp = tempfile::tempdir().unwrap();
    // Lower the candidate similarity threshold so this test is independent of
    // the default embedder's absolute similarity scale.
    let mut cfg = Config::default();
    cfg.conflict.similarity_threshold = 0.4;
    let mut b = Brain::open(tmp.path(), cfg).unwrap();

    let old_text =
        "Deployment orchestration uses Kubernetes on GCP with three worker nodes and ArgoCD";
    let first = match b.remember(old_text, RememberOptions::default()).unwrap() {
        RememberOutcome::Stored { id } => id,
        other => panic!("expected stored: {other:?}"),
    };

    // Superset of the old text (guarantees high vector similarity) plus an
    // explicit supersede signal — same author, so this is a Supersede conflict.
    let new_text = format!("{old_text}; we now migrate to Docker Swarm instead");
    let out = b.remember(&new_text, RememberOptions::default()).unwrap();

    match out {
        RememberOutcome::NeedsReview { conflict_id, .. } => {
            let r = b
                .resolve(&conflict_id, ResolutionAction::Replace, None, None, None)
                .unwrap();
            assert_eq!(r.archived_ids, vec![first.clone()]);
            let old = b.get(&first).unwrap().unwrap();
            assert_eq!(old.state, MemoryState::Superseded);
        }
        RememberOutcome::AutoResolved { .. } => {
            // Auto-resolution is also acceptable; verify old is superseded.
            let old = b.get(&first).unwrap().unwrap();
            assert_eq!(old.state, MemoryState::Superseded);
        }
        RememberOutcome::Stored { .. } => panic!("expected a conflict to be detected"),
    }
}

#[test]
fn export_import_roundtrip() {
    let src = tempfile::tempdir().unwrap();
    let jsonl = {
        let mut b = brain_at(src.path());
        b.remember("Auth uses JWT tokens", RememberOptions::default())
            .unwrap();
        b.remember(
            "Database is PostgreSQL with a read replica",
            RememberOptions::default(),
        )
        .unwrap();
        b.save().unwrap();
        b.export(None)
            .unwrap()
            .iter()
            .map(|m| serde_json::to_string(m).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
    };

    // Import into a fresh brain.
    let dst = tempfile::tempdir().unwrap();
    let mut b2 = brain_at(dst.path());
    let mut stored = 0;
    for line in jsonl.lines() {
        let m = serde_json::from_str(line).unwrap();
        if b2.import_memory(m).unwrap() {
            stored += 1;
        }
    }
    assert_eq!(stored, 2);
    // Re-import is idempotent (dedup by id).
    for line in jsonl.lines() {
        let m = serde_json::from_str(line).unwrap();
        assert!(!b2.import_memory(m).unwrap());
    }

    let res = b2
        .recall("authentication", 5, DetailLevel::Summary, 200, None)
        .unwrap();
    assert!(!res.output.ids.is_empty());
}

#[test]
fn recall_respects_token_budget() {
    let tmp = tempfile::tempdir().unwrap();
    let mut b = brain_at(tmp.path());
    for i in 0..30 {
        b.remember(
            &format!(
                "Memory {i}: the system uses component number {i} for processing data pipelines"
            ),
            RememberOptions::default(),
        )
        .ok();
    }
    let res = b
        .recall("system component data", 20, DetailLevel::Summary, 120, None)
        .unwrap();
    assert!(
        res.output.tokens_used <= 120,
        "budget exceeded: {}",
        res.output.tokens_used
    );
}
