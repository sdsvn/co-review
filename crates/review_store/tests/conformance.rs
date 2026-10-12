//! Conformance test: runs the same sequence of operations as gen/gen.mjs on the
//! TypeScript ReviewStore and verifies the Rust store produces compatible JSON.
//!
//! Since gen.mjs generates different UUIDs and timestamps each run, we compare
//! structurally: same fields, same field names (camelCase), same types, same
//! invariants (thread numbers, message counts, etc.).

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use review_model::*;
    use review_store::*;
    fn test_author() -> Participant {
        Participant {
            id: "human:test@test.com".into(),
            kind: ParticipantKind::Human,
            name: "Test User".into(),
        }
    }

    fn test_location() -> CodeLocation {
        CodeLocation {
            kind: LocationKind::Repository,
            uri: None,
            range: None,
            symbol: None,
            anchor: None,
            doc_anchor: None,
            patch_anchor: None,
            tracked: None,
            extra: serde_json::Map::new(),
        }
    }

    /// Run the same operations as gen.mjs and verify the output.
    #[test]
    fn test_operations_match_ts() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ReviewStore::with_home(tmp.path().to_path_buf());
        let workspace_root = "file:///nonexistent_test_workspace_42/project";

        // Verify workspace hash matches TS
        let ws_dir = store.workspace_dir(workspace_root);
        let ws_hash = ws_dir.file_name().unwrap().to_str().unwrap();
        assert_eq!(ws_hash, "f1d58960111d5c78", "workspace hash must match TS");

        // 1. Create a review
        let review = store
            .create(CreateReviewParams {
                workspace_root: workspace_root.to_string(),
                title: "Test Review".into(),
                scope: ReviewScope::Repository,
                author: test_author(),
            })
            .unwrap();
        assert_eq!(review.title, "Test Review");
        assert_eq!(review.next_thread_number, 1);
        assert_eq!(review.activity.len(), 1);
        assert_eq!(review.activity[0].kind, ActivityKind::ReviewCreated);

        // 2. List reviews
        let listed = store.list(workspace_root);
        assert_eq!(listed.len(), 1);

        // 3. Rename
        let renamed = store.rename(&review.id, "Renamed Review".into()).unwrap();
        assert_eq!(renamed.title, "Renamed Review");

        // 4. Create a thread
        let (updated, thread) = store
            .create_thread(
                &review.id,
                test_location(),
                "First comment".into(),
                test_author(),
                ThreadOptions::default(),
            )
            .unwrap();
        assert_eq!(thread.number, 1);
        assert_eq!(thread.messages.len(), 1);
        assert_eq!(thread.messages[0].body, "First comment");
        assert_eq!(thread.status, ThreadStatus::Open);
        assert_eq!(thread.intent, ThreadIntent::Comment);
        assert_eq!(updated.next_thread_number, 2);

        // 5. Add a message
        let thread2 = store
            .add_message(
                &review.id,
                &thread.id,
                "Reply message".into(),
                test_author(),
                None,
            )
            .unwrap();
        assert_eq!(thread2.messages.len(), 2);
        assert_eq!(thread2.messages[1].body, "Reply message");

        // 6. Set thread status
        let resolved = store
            .set_thread_status(
                &review.id,
                &thread.id,
                ThreadStatus::Resolved,
                test_author(),
            )
            .unwrap();
        assert_eq!(resolved.status, ThreadStatus::Resolved);

        // 7. Archive
        let archived = store.archive(&review.id, true).unwrap();
        assert!(archived.archived_at.is_some());

        // 8. Unarchive
        let unarchived = store.archive(&review.id, false).unwrap();
        assert!(unarchived.archived_at.is_none());

        // 9. Set viewed
        let viewed = store
            .set_viewed(
                &review.id,
                &["src/main.rs".into(), "src/lib.rs".into()],
                true,
            )
            .unwrap();
        let viewed_map = viewed.viewed.as_ref().unwrap();
        assert!(viewed_map.contains_key("src/main.rs"));
        assert!(viewed_map.contains_key("src/lib.rs"));

        // 10. Submit
        let submitted = store
            .submit(
                &review.id,
                ReviewDecision::Approve,
                "LGTM".into(),
                test_author(),
            )
            .unwrap();
        let verdict = submitted.verdict.as_ref().unwrap();
        assert_eq!(verdict.decision, ReviewDecision::Approve);
        assert_eq!(verdict.count, 1);
        assert_eq!(verdict.summary, "LGTM");

        // 11. Set overview
        let with_overview = store
            .set_overview(&review.id, "# Overview\nThis is a test.".into())
            .unwrap();
        assert_eq!(
            with_overview.overview.as_deref(),
            Some("# Overview\nThis is a test.")
        );

        // Verify the final JSON has camelCase field names (TS compatibility)
        let final_review = store.get(&review.id).unwrap();
        let json_value = serde_json::to_value(&final_review).unwrap();

        // Check key field names are camelCase
        let obj = json_value.as_object().unwrap();
        assert!(obj.contains_key("workspaceRoot"), "missing workspaceRoot");
        assert!(obj.contains_key("createdAt"), "missing createdAt");
        assert!(obj.contains_key("updatedAt"), "missing updatedAt");
        assert!(
            obj.contains_key("nextThreadNumber"),
            "missing nextThreadNumber"
        );

        // Thread fields
        let threads = obj["threads"].as_array().unwrap();
        let t = threads[0].as_object().unwrap();
        assert!(t.contains_key("createdAt"), "thread missing createdAt");
        assert!(t.contains_key("updatedAt"), "thread missing updatedAt");

        // Message fields
        let msgs = t["messages"].as_array().unwrap();
        let m = msgs[0].as_object().unwrap();
        assert!(m.contains_key("createdAt"), "message missing createdAt");

        // Verify the review can be read back from disk (round-trip)
        let file = store
            .workspace_dir(workspace_root)
            .join("reviews")
            .join(format!("{}.json", review.id));
        let raw = std::fs::read_to_string(&file).unwrap();
        let parsed: Review = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.id, review.id);
        assert_eq!(parsed.title, "Renamed Review");

        // Verify workspace.json was written
        let ws_info_file = store.workspace_dir(workspace_root).join("workspace.json");
        let ws_info: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&ws_info_file).unwrap()).unwrap();
        assert_eq!(ws_info["root"].as_str().unwrap(), workspace_root);
    }

    /// Verify a review written by Rust can be parsed as the TS model would expect.
    #[test]
    fn test_json_shape_matches_ts() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ReviewStore::with_home(tmp.path().to_path_buf());
        let review = store
            .create(CreateReviewParams {
                workspace_root: "file:///test".into(),
                title: "Shape Test".into(),
                scope: ReviewScope::Repository,
                author: test_author(),
            })
            .unwrap();

        let json = serde_json::to_string_pretty(&review).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let obj = value.as_object().unwrap();

        // The TS model expects these exact top-level fields
        let required_fields = [
            "id",
            "title",
            "workspaceRoot",
            "scope",
            "participants",
            "threads",
            "activity",
            "nextThreadNumber",
            "createdAt",
            "updatedAt",
        ];
        for field in required_fields {
            assert!(
                obj.contains_key(field),
                "missing required field: {field}"
            );
        }

        // Optional fields should NOT be present when None (skip_serializing_if)
        assert!(!obj.contains_key("agent"), "agent should be omitted when None");
        assert!(
            !obj.contains_key("bundle"),
            "bundle should be omitted when None"
        );
        assert!(
            !obj.contains_key("overview"),
            "overview should be omitted when None"
        );
        assert!(
            !obj.contains_key("verdict"),
            "verdict should be omitted when None"
        );
        assert!(
            !obj.contains_key("closed"),
            "closed should be omitted when None"
        );
        assert!(
            !obj.contains_key("archivedAt"),
            "archivedAt should be omitted when None"
        );
        assert!(
            !obj.contains_key("viewed"),
            "viewed should be omitted when None"
        );
    }
}
