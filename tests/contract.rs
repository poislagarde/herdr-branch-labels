use herdr_branch_labels::{
    current_hint, formatter::Formatter, indented_workspaces, labels, saved_workspaces, token_text,
};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

fn configured() -> Formatter {
    Formatter::new(&json!({"pattern": "^feature/"})).unwrap()
}

fn workspace() -> Value {
    json!({"workspace_id": "w1", "label": "checkout-directory"})
}

fn worktree(id: &str, repo: &str, linked: bool) -> Value {
    json!({"workspace_id": id, "worktree": {"repo_key": repo, "is_linked_worktree": linked}})
}

struct SnapshotDirectory(PathBuf);

impl SnapshotDirectory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "branch-labels-contract-{}-{timestamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn socket(&self) -> PathBuf {
        self.0.join("herdr.sock")
    }

    fn write(&self, content: &str) {
        std::fs::write(self.0.join("session.json"), content).unwrap();
    }
}

impl Drop for SnapshotDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn missing_or_null_pattern_preserves_input_exactly() {
    for config in [
        json!({}),
        json!({"pattern": null}),
        json!({"replacement": "$1"}),
    ] {
        let formatter = Formatter::new(&config).unwrap();
        for branch in ["main", "feature/invoices", "  retain whitespace  ", ""] {
            assert_eq!(formatter.format(branch).unwrap(), branch);
        }
    }
}

#[test]
fn configuration_rejects_unknown_keys_and_wrong_types() {
    for config in [
        json!(null),
        json!([]),
        json!({"typo": "value"}),
        json!({"pattern": 1}),
        json!({"replacement": null}),
        json!({"pattern": null, "replacement": false}),
        json!({"pattern": "["}),
    ] {
        assert!(Formatter::new(&config).is_err(), "accepted {config}");
    }
}

#[test]
fn consumer_date_prefix_config_accepts_lookahead_without_changing_empty_suffixes() {
    let formatter = Formatter::new(&json!({
        "pattern": r"^[^/]+/[0-9]{4}-[0-9]{2}-[0-9]{2}-(?=.)",
        "replacement": "",
    }))
    .unwrap();
    assert_eq!(
        formatter
            .format("feat/2026-09-08-invoice-validation")
            .unwrap(),
        "invoice-validation"
    );
    for branch in ["main", "chore/2026-09-10-", "team/feat/2026-09-10-invoices"] {
        assert_eq!(formatter.format(branch).unwrap(), branch);
    }
}

#[test]
fn only_first_match_is_replaced_and_empty_results_fall_back() {
    let formatter = Formatter::new(&json!({"pattern": "foo"})).unwrap();
    assert_eq!(formatter.format("foo-foo").unwrap(), "-foo");
    assert_eq!(formatter.format("foo").unwrap(), "foo");
    assert_eq!(configured().format("feature/").unwrap(), "feature/");
}

#[test]
fn rust_capture_replacements_support_numbered_and_named_groups() {
    let formatter = Formatter::new(&json!({
        "pattern": r"^users/[^/]+/(?P<ticket>ABC-\d+)-(.+)$",
        "replacement": "${2} (${ticket})",
    }))
    .unwrap();
    assert_eq!(
        formatter.format("users/alice/ABC-42-invoices").unwrap(),
        "invoices (ABC-42)"
    );
    let formatter =
        Formatter::new(&json!({"pattern": "^feature/(.+)$", "replacement": "$1"})).unwrap();
    assert_eq!(formatter.format("feature/invoices").unwrap(), "invoices");
}

#[test]
fn grouped_automatic_spaces_use_actual_branch() {
    assert_eq!(
        labels(
            &workspace(),
            Some(&json!({"custom_name": null})),
            Some("feature/real-description"),
            true,
            &configured(),
            false
        )
        .unwrap(),
        json!({"short_space": "real-description", "short_branch": null}),
    );
}

#[test]
fn unconfigured_grouped_spaces_keep_full_branch() {
    let formatter = Formatter::new(&json!({})).unwrap();
    assert_eq!(
        labels(
            &workspace(),
            Some(&json!({"custom_name": null})),
            Some("feature/real-description"),
            true,
            &formatter,
            false
        )
        .unwrap(),
        json!({"short_space": "feature/real-description", "short_branch": null}),
    );
}

#[test]
fn root_spaces_retain_directory_name_and_format_branch_metadata() {
    assert_eq!(
        labels(
            &workspace(),
            Some(&json!({"custom_name": null})),
            Some("feature/real-description"),
            false,
            &configured(),
            false
        )
        .unwrap(),
        json!({"short_space": "checkout-directory", "short_branch": "real-description"}),
    );
}

#[test]
fn manual_names_and_unknown_identity_hints_are_preserved() {
    let workspace = json!({"workspace_id": "w1", "label": "feature/manual-name"});
    let hints = [json!({}), json!({"custom_name": "feature/manual-name"})];
    for hint in [None, Some(&hints[0]), Some(&hints[1])] {
        assert_eq!(
            labels(
                &workspace,
                hint,
                Some("feature/branch-name"),
                true,
                &configured(),
                false
            )
            .unwrap(),
            json!({"short_space": "feature/manual-name", "short_branch": null}),
        );
    }
}

#[test]
fn live_rename_wins_over_automatic_saved_hint() {
    assert_eq!(
        labels(
            &workspace(),
            Some(&json!({"custom_name": null})),
            Some("feature/branch-name"),
            true,
            &configured(),
            true
        )
        .unwrap()["short_space"],
        "checkout-directory",
    );
}

#[test]
fn detached_and_non_git_spaces_clear_branch_token() {
    for branch in [None, Some("")] {
        for indented in [false, true] {
            assert_eq!(
                labels(&workspace(), None, branch, indented, &configured(), false).unwrap(),
                json!({"short_space": "checkout-directory", "short_branch": null}),
            );
        }
    }
}

#[test]
fn token_normalization_removes_controls_and_collapses_visible_whitespace() {
    assert_eq!(
        token_text(Some(" a\x00b\x7f\u{85}\u{9f}  c\u{a0}d ")),
        Some("ab c d".into())
    );
    assert_eq!(token_text(Some("a\nb\tc")), Some("abc".into()));
    assert_eq!(token_text(Some(" \x00 ")), None);
    assert_eq!(token_text(None), None);
}

#[test]
fn token_limit_counts_unicode_characters_instead_of_bytes() {
    assert_eq!(token_text(Some(&"λ".repeat(100))), Some("λ".repeat(80)));
    assert_eq!(token_text(Some(&"🦀".repeat(100))), Some("🦀".repeat(80)));
}

#[test]
fn grouping_indents_linked_and_additional_primary_spaces() {
    let workspaces = [
        worktree("child", "repo", true),
        worktree("parent", "repo", false),
        worktree("parent2", "repo", false),
        worktree("orphan", "other", true),
    ];
    assert_eq!(
        indented_workspaces(&workspaces).unwrap(),
        BTreeSet::from(["child".to_owned(), "parent2".to_owned()])
    );
}

#[test]
fn grouping_without_a_primary_or_repository_stays_flat() {
    let workspaces = [
        worktree("one", "repo", true),
        worktree("two", "repo", true),
        json!({"workspace_id": "none"}),
        json!({"workspace_id": "null", "worktree": null}),
        worktree("empty-key", "", false),
    ];
    assert!(indented_workspaces(&workspaces).unwrap().is_empty());
}

#[test]
fn separate_repository_groups_have_independent_parents() {
    let workspaces = [
        worktree("parent1", "one", false),
        worktree("child1", "one", true),
        worktree("parent2", "two", false),
        worktree("child2", "two", true),
    ];
    assert_eq!(
        indented_workspaces(&workspaces).unwrap(),
        BTreeSet::from(["child1".to_owned(), "child2".to_owned()])
    );
}

#[test]
fn saved_custom_names_must_match_live_labels() {
    let workspace = json!({"label": "manual"});
    assert!(current_hint(
        &workspace,
        Some(&json!({"custom_name": "manual"}))
    ));
    assert!(!current_hint(
        &workspace,
        Some(&json!({"custom_name": "old-name"}))
    ));
    assert!(!current_hint(&workspace, Some(&json!({}))));
    assert!(!current_hint(&workspace, None));
}

#[test]
fn automatic_identity_requires_matching_absolute_directory() {
    let workspace = json!({"label": "checkout"});
    assert!(current_hint(
        &workspace,
        Some(&json!({"custom_name": null, "identity_cwd": "/repo/checkout"}))
    ));
    assert!(current_hint(
        &workspace,
        Some(&json!({"custom_name": null, "identity_cwd": "/repo/checkout/"}))
    ));
    assert!(!current_hint(
        &workspace,
        Some(&json!({"custom_name": null, "identity_cwd": "repo/checkout"}))
    ));
    assert!(!current_hint(
        &workspace,
        Some(&json!({"custom_name": null, "identity_cwd": "/repo/other"}))
    ));
    assert!(!current_hint(
        &workspace,
        Some(&json!({"custom_name": null}))
    ));
}

#[test]
fn absent_invalid_and_unsupported_snapshots_return_no_hints() {
    let directory = SnapshotDirectory::new();
    assert!(saved_workspaces(&directory.socket()).is_empty());
    for content in [
        "{",
        "[]",
        "null",
        "{}",
        r#"{"version":4,"workspaces":[]}"#,
        r#"{"version":3,"workspaces":null}"#,
    ] {
        directory.write(content);
        assert!(
            saved_workspaces(&directory.socket()).is_empty(),
            "accepted {content}"
        );
    }
}

#[test]
fn v3_snapshots_read_explicit_identity_and_ignore_invalid_entries() {
    let directory = SnapshotDirectory::new();
    let workspace = json!({"id": "w1", "custom_name": "mine", "identity_cwd": "/repo"});
    directory.write(
        &json!({"version": 3, "workspaces": [null, 4, {}, {"id": 1}, workspace]}).to_string(),
    );
    let saved = saved_workspaces(&directory.socket());
    assert_eq!(saved.len(), 1);
    assert_eq!(saved["w1"], workspace);
}

#[test]
fn duplicate_snapshot_ids_use_the_latest_entry() {
    let directory = SnapshotDirectory::new();
    directory.write(
        &json!({"version": 3, "workspaces": [
            {"id": "w1", "custom_name": "old"}, {"id": "w1", "custom_name": "current"},
        ]})
        .to_string(),
    );
    assert_eq!(
        saved_workspaces(&directory.socket())["w1"]["custom_name"],
        "current"
    );
}
