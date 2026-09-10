import json
from pathlib import Path
import re
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import plugin


class FormattingTests(unittest.TestCase):
    def test_missing_or_null_pattern_preserves_exact_input(self):
        for config in ({}, {"pattern": None}, {"replacement": "unused"},
                       {"pattern": None, "replacement": r"\1"}):
            format_branch = plugin.formatter(config)
            for branch in ("main", "feature/invoices", "ticket/ABC-42-invoices",
                           "  retain whitespace  ", ""):
                with self.subTest(config=config, branch=branch):
                    self.assertEqual(format_branch(branch), branch)

    def test_explicit_pattern_defaults_to_empty_replacement(self):
        self.assertEqual(plugin.formatter({"pattern": r"^feature/"})("feature/invoices"),
                         "invoices")

    def test_unmatched_and_empty_description_are_preserved(self):
        format_branch = plugin.formatter({"pattern": r"^feature/"})
        for branch in ("main", "ticket/invoices", "feature/", "team/feature/invoices"):
            self.assertEqual(format_branch(branch), branch)

    def test_custom_pattern_and_capture_replacement(self):
        format_branch = plugin.formatter({
            "pattern": r"^users/[^/]+/(ABC-\d+)-(.+)$",
            "replacement": r"\2 (\1)",
        })
        self.assertEqual(format_branch("users/alice/ABC-42-invoices"), "invoices (ABC-42)")

    def test_only_first_match_replaced(self):
        self.assertEqual(plugin.formatter({"pattern": "foo"})("foo-foo"), "-foo")

    def test_empty_transformation_falls_back(self):
        self.assertEqual(plugin.formatter({"pattern": ".*"})("main"), "main")

    def test_invalid_configuration(self):
        for config in ([], {"typo": "value"}, {"pattern": 1}, {"replacement": None},
                       {"pattern": None, "replacement": 1},
                       {"pattern": "["}, {"pattern": "a", "replacement": r"\2"}):
            with self.subTest(config=config), self.assertRaises((ValueError, re.error)):
                plugin.formatter(config)


class LabelTests(unittest.TestCase):
    def setUp(self):
        self.workspace = {"workspace_id": "w1", "label": "checkout-name"}
        self.branch = "feature/real-description"
        self.formatter = plugin.formatter({"pattern": r"^feature/"})

    def labels(self, hint, indented=True, **kwargs):
        return plugin.labels(self.workspace, hint, self.branch, indented, self.formatter, **kwargs)

    def test_grouped_automatic_space_uses_actual_branch(self):
        self.assertEqual(self.labels({"custom_name": None}),
                         {"short_space": "real-description", "short_branch": None})

    def test_unconfigured_labels_preserve_full_branch(self):
        for indented in (True, False):
            with self.subTest(indented=indented):
                result = plugin.labels(self.workspace, {"custom_name": None}, self.branch,
                                       indented, plugin.formatter({}))
                self.assertEqual(result, {
                    "short_space": self.branch if indented else "checkout-name",
                    "short_branch": None if indented else self.branch,
                })

    def test_root_retains_repo_name_and_formats_branch(self):
        self.assertEqual(self.labels({"custom_name": None}, indented=False),
                         {"short_space": "checkout-name", "short_branch": "real-description"})

    def test_custom_names_and_unknown_hints_are_preserved(self):
        for hint in (None, {}, {"custom_name": "checkout-name"}):
            self.assertEqual(self.labels(hint)["short_space"], "checkout-name")

    def test_live_rename_wins_over_old_saved_hint(self):
        self.assertEqual(self.labels({"custom_name": None}, renamed=True)["short_space"],
                         "checkout-name")

    def test_regex_receives_exact_branch_for_grouped_spaces(self):
        result = plugin.labels(self.workspace, {"custom_name": None}, "worktree/feature",
                               True, plugin.formatter({"pattern": r"^worktree/(.+)$",
                                                       "replacement": r"task-\1"}))
        self.assertEqual(result["short_space"], "task-feature")

    def test_detached_or_non_git_space_has_no_branch(self):
        result = plugin.labels(self.workspace, None, None, False, self.formatter)
        self.assertEqual(result, {"short_space": "checkout-name", "short_branch": None})

    def test_grouping_matches_herdr(self):
        def workspace(id, key, linked):
            return {"workspace_id": id, "worktree": {
                "repo_key": key, "is_linked_worktree": linked,
            }}
        workspaces = [workspace("child", "repo", True), workspace("parent", "repo", False),
                      workspace("parent2", "repo", False), workspace("orphan", "other", True)]
        self.assertEqual(plugin.indented_workspaces(workspaces), {"child", "parent2"})

    def test_token_values_fit_herdr_limits(self):
        self.assertEqual(plugin.token_text("λ" * 100), "λ" * 80)
        self.assertIsNone(plugin.token_text(" \x00 "))


class SavedIdentityTests(unittest.TestCase):
    def test_stale_name_hints_are_rejected(self):
        hint = {"custom_name": None, "identity_cwd": "/repo/checkout"}
        self.assertFalse(plugin.current_hint({"label": "my custom label"}, hint))
        self.assertTrue(plugin.current_hint({"label": "checkout"}, hint))
        self.assertTrue(plugin.current_hint({"label": "my custom label"},
                                           dict(hint, custom_name="my custom label")))

    def test_missing_unsupported_and_invalid_snapshots_fall_back(self):
        with tempfile.TemporaryDirectory() as tmp:
            socket = Path(tmp) / "herdr.sock"
            path = socket.with_name("session.json")
            self.assertEqual(plugin.saved_workspaces(socket), {})
            for content in ("{", '[]', '{"version":4,"workspaces":[]}',
                            '{"version":3,"workspaces":null}'):
                path.write_text(content)
                self.assertEqual(plugin.saved_workspaces(socket), {})

    def test_reads_explicit_session_identity_hints(self):
        with tempfile.TemporaryDirectory() as tmp:
            socket = Path(tmp) / "herdr.sock"
            workspace = {"id": "w1", "custom_name": "mine", "identity_cwd": "/repo"}
            socket.with_name("session.json").write_text(json.dumps({
                "version": 3, "workspaces": [workspace],
            }))
            self.assertEqual(plugin.saved_workspaces(socket), {"w1": workspace})


class PublishingTests(unittest.TestCase):
    def test_fallback_write_does_not_hide_later_formatted_update(self):
        with patch.dict("os.environ", {"HERDR_SOCKET_PATH": "/tmp/test.sock"}):
            herdr = plugin.Herdr()
        workspace = {"workspace_id": "w1", "tokens": {"short_space": "description"}}
        with patch.object(herdr, "call") as call:
            herdr.publish(workspace, {"short_space": "checkout"})
            herdr.publish(workspace, {"short_space": "description"})
            self.assertEqual(call.call_count, 2)
            self.assertEqual(workspace["tokens"]["short_space"], "description")

    def test_unchanged_metadata_does_not_emit_writes(self):
        with patch.dict("os.environ", {"HERDR_SOCKET_PATH": "/tmp/test.sock"}):
            herdr = plugin.Herdr()
        with patch.object(herdr, "call") as call:
            self.assertFalse(herdr.publish({"tokens": {"short_space": "main"}},
                                           {"short_space": "main", "short_branch": None}))
            call.assert_not_called()


if __name__ == "__main__":
    unittest.main()
