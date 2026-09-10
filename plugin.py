#!/usr/bin/env python3
"""Publish display-only sidebar labels; never rename a workspace or Git branch."""

import argparse
from concurrent.futures import ThreadPoolExecutor
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time


SOURCE = "plugin:poislagarde.branch-labels"


def formatter(config):
    """Compile configuration before making any metadata changes."""
    if not isinstance(config, dict) or set(config) - {"pattern", "replacement"}:
        raise ValueError("config.json accepts only pattern and replacement")
    pattern = config.get("pattern")
    replacement = config.get("replacement", "")
    if not isinstance(replacement, str):
        raise ValueError("replacement must be a string")
    if pattern is None:
        return lambda branch: branch
    if not isinstance(pattern, str):
        raise ValueError("pattern must be a string or null")
    regex = re.compile(pattern)
    # Validate replacement references even when no branch matches the pattern.
    regex.sub(replacement, "")

    def format_branch(branch):
        return regex.sub(replacement, branch, count=1).strip() or branch

    return format_branch


def saved_workspaces(socket_path):
    """Read only the v3 identity/custom-name hints missing from WorkspaceInfo."""
    try:
        snapshot = json.loads(socket_path.with_name("session.json").read_text())
        if snapshot.get("version") != 3:
            return {}
        return {
            item["id"]: item for item in snapshot["workspaces"]
            if isinstance(item, dict) and isinstance(item.get("id"), str)
        }
    except (OSError, ValueError, KeyError, TypeError, AttributeError):
        return {}


def current_hint(workspace, hint):
    """Reject saved names that have not caught up with a live rename."""
    if not hint or "custom_name" not in hint:
        return False
    name = hint["custom_name"]
    if name is None:
        cwd = hint.get("identity_cwd")
        if not isinstance(cwd, str) or not os.path.isabs(cwd):
            return False
        name = Path(cwd).name or cwd
    return name == workspace["label"]


def identity_hints(herdr, workspaces):
    # Herdr debounces session saves for five seconds. New/renamed spaces need
    # that save before the API's missing custom-name flag can be trusted.
    deadline = time.monotonic() + 6
    initialized = False
    changed = 0
    while True:
        saved = saved_workspaces(herdr.socket_path)
        hints = {item["workspace_id"]: saved.get(item["workspace_id"])
                 for item in workspaces
                 if current_hint(item, saved.get(item["workspace_id"]))}
        if not initialized:
            # Keep new entries readable while Herdr saves their identity hints.
            for item in workspaces:
                if item["workspace_id"] not in hints:
                    changed += herdr.publish(item, {"short_space": token_text(item["label"])})
            initialized = True
        if len(hints) == len(workspaces) or time.monotonic() >= deadline:
            return hints, changed
        time.sleep(0.1)


def indented_workspaces(workspaces):
    """Match Herdr's group rule, including multiple primary spaces in one repo."""
    groups = {}
    for workspace in workspaces:
        worktree = workspace.get("worktree") or {}
        if worktree.get("repo_key"):
            groups.setdefault(worktree["repo_key"], []).append(workspace)
    indented = set()
    for members in groups.values():
        parent = next((item for item in members
                       if not item["worktree"]["is_linked_worktree"]), None)
        if len(members) > 1 and parent is not None:
            indented.update(item["workspace_id"] for item in members if item is not parent)
    return indented


def branch_at(path):
    if not isinstance(path, str) or not os.path.isabs(path):
        return None
    env = os.environ.copy()
    for key in ("GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR", "GIT_NAMESPACE"):
        env.pop(key, None)
    env["GIT_OPTIONAL_LOCKS"] = "0"
    try:
        result = subprocess.run(
            ["git", "-C", path, "symbolic-ref", "--quiet", "--short", "HEAD"],
            stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=3,
            env=env,
        )
        return (result.stdout.strip() or None) if result.returncode == 0 else None
    except (OSError, subprocess.TimeoutExpired):
        return None


def token_text(value):
    # Match Herdr's metadata normalization so repeated refreshes are no-ops.
    if value is None:
        return None
    return " ".join("".join(char for char in value if not ord(char) < 32
                            and not 127 <= ord(char) <= 159).split())[:80] or None


def labels(workspace, hint, branch, indented, format_branch, renamed=False):
    name = workspace["label"]
    # Only a known automatic name may be replaced by the branch-derived label.
    # The live API name wins on rename events while the saved file catches up.
    automatic = hint is not None and "custom_name" in hint and hint["custom_name"] is None
    if indented and automatic and branch and not renamed:
        name = format_branch(branch)
    return {
        "short_space": token_text(name),
        "short_branch": token_text(format_branch(branch)) if branch and not indented else None,
    }


class Herdr:
    def __init__(self):
        path = os.environ.get("HERDR_SOCKET_PATH", "")
        if not os.path.isabs(path):
            raise ValueError("HERDR_SOCKET_PATH must identify an explicit Herdr session")
        self.socket_path = Path(path)
        self.binary = os.environ.get("HERDR_BIN_PATH", "herdr")
        self.env = os.environ.copy()
        self.env.pop("HERDR_SESSION", None)

    def call(self, *args):
        result = subprocess.run(
            [self.binary, *args], env=self.env, stdin=subprocess.DEVNULL,
            capture_output=True, text=True, timeout=8,
        )
        if result.returncode:
            raise RuntimeError(result.stderr.strip() or "Herdr command failed")
        if args[:2] == ("workspace", "report-metadata") and not result.stdout.strip():
            return {}
        response = json.loads(result.stdout)
        if "error" in response:
            raise RuntimeError(str(response["error"]))
        return response["result"]

    def publish(self, workspace, tokens):
        old = workspace.get("tokens", {})
        if all(old.get(key) == value for key, value in tokens.items()):
            return False
        args = ["workspace", "report-metadata", workspace["workspace_id"], "--source", SOURCE]
        for key, value in tokens.items():
            args.extend(["--clear-token", key] if value is None
                        else ["--token", key + "=" + value])
        try:
            self.call(*args)
        except RuntimeError:
            # A space may close between the snapshot and the metadata write.
            live = self.call("workspace", "list")["workspaces"]
            if any(item["workspace_id"] == workspace["workspace_id"] for item in live):
                raise
            return False
        current = workspace.setdefault("tokens", {})
        for key, value in tokens.items():
            if value is None:
                current.pop(key, None)
            else:
                current[key] = value
        return True


def refresh():
    herdr = Herdr()
    config_dir = Path(os.environ["HERDR_PLUGIN_CONFIG_DIR"])
    try:
        config = json.loads((config_dir / "config.json").read_text())
    except FileNotFoundError:
        config = {}
    format_branch = formatter(config)
    state_dir = Path(os.environ["HERDR_PLUGIN_STATE_DIR"])
    state_dir.mkdir(parents=True, exist_ok=True)
    session_key = hashlib.sha256(os.fsencode(herdr.socket_path)).hexdigest()[:24]
    with (state_dir / (session_key + ".lock")).open("w") as lock:
        # Serialize overlapping lifecycle hooks, then read current state.
        fcntl.flock(lock, fcntl.LOCK_EX)
        workspaces = herdr.call("workspace", "list")["workspaces"]
        saved, changed = identity_hints(herdr, workspaces)
        indented = indented_workspaces(workspaces)
        renamed_id = None
        if os.environ.get("HERDR_PLUGIN_EVENT") == "workspace.renamed":
            context = json.loads(os.environ.get("HERDR_PLUGIN_CONTEXT_JSON", "{}"))
            renamed_id = context.get("workspace_id")
        paths = []
        for workspace in workspaces:
            worktree = workspace.get("worktree") or {}
            hint = saved.get(workspace["workspace_id"], {})
            paths.append(worktree.get("checkout_path") or hint.get("identity_cwd"))
        with ThreadPoolExecutor(max_workers=4) as pool:
            branches = list(pool.map(branch_at, paths))
        for workspace, branch in zip(workspaces, branches):
            workspace_id = workspace["workspace_id"]
            tokens = labels(workspace, saved.get(workspace_id), branch,
                            workspace_id in indented, format_branch,
                            renamed=workspace_id == renamed_id)
            changed += herdr.publish(workspace, tokens)
        return changed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["refresh"])
    parser.parse_args()
    try:
        print(json.dumps({"updated": refresh()}))
    except (OSError, ValueError, RuntimeError, KeyError, re.error,
            subprocess.TimeoutExpired) as error:
        print("branch-labels: " + str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
