#!/usr/bin/env python3
"""Verify the plugin on disposable headless Herdr servers and Git worktrees.

Requires Python 3.9+, Git, and Herdr 0.9.0+. All config, state, sockets,
repositories, and shell startup files live under a private temporary directory.
"""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time


PLUGIN_ID = "poislagarde.branch-labels"
TIMEOUT = 20


def command(argv, cwd, env):
    result = subprocess.run(
        [str(arg) for arg in argv], cwd=cwd, env=env,
        capture_output=True, text=True, timeout=TIMEOUT,
    )
    if result.returncode:
        raise AssertionError("{} failed ({})\n{}\n{}".format(
            argv, result.returncode, result.stdout, result.stderr))
    return result.stdout.strip()


def wait_for(description, predicate):
    deadline = time.monotonic() + TIMEOUT
    last_error = None
    while time.monotonic() < deadline:
        try:
            result = predicate()
            if result:
                return result
        except (OSError, ValueError, KeyError) as error:
            last_error = error
        time.sleep(0.05)
    raise AssertionError("Timed out waiting for {}; last error: {}".format(description, last_error))


class Server:
    def __init__(self, root, herdr, env):
        self.root, self.herdr = root, herdr
        self.env = env.copy()
        self.socket = root / "config" / "herdr" / "herdr.sock"
        self.env["HERDR_SOCKET_PATH"] = str(self.socket)
        self.log_path = root / "server.log"
        self.log_file = self.log_path.open("a")
        self.process = subprocess.Popen(
            [herdr, "server"], cwd=root, env=self.env, stdin=subprocess.DEVNULL,
            stdout=self.log_file, stderr=subprocess.STDOUT, start_new_session=True,
        )

    def run(self, *args):
        assert self.socket.is_relative_to(self.root)
        value = json.loads(command([self.herdr] + list(args), self.root, self.env))
        assert not value.get("error"), value
        return value["result"]

    def ready(self):
        def check():
            if self.process.poll() is not None:
                raise AssertionError("Private server exited: " + self.log_path.read_text())
            return self.socket.exists() and self.run("workspace", "list")
        wait_for("private server startup", check)

    def workspace(self, workspace_id):
        return self.run("workspace", "get", workspace_id)["workspace"]

    def tokens(self, workspace_id, space, branch=None):
        def check():
            tokens = self.workspace(workspace_id).get("tokens", {})
            return tokens.get("short_space") == space and tokens.get("short_branch") == branch
        wait_for("tokens {!r}/{!r} for {}".format(space, branch, workspace_id), check)

    def refresh(self):
        result = self.run("plugin", "action", "invoke", "refresh", "--plugin", PLUGIN_ID)
        return result

    def saved(self, workspace_id, custom_name=None):
        snapshot_path = self.socket.parent / "session.json"
        def check():
            workspaces = json.loads(snapshot_path.read_text())["workspaces"]
            return any(workspace.get("id") == workspace_id
                       and workspace.get("custom_name") == custom_name for workspace in workspaces)
        wait_for("saved workspace " + workspace_id, check)

    def logs(self):
        return self.run("plugin", "log", "list", "--plugin", PLUGIN_ID, "--limit", "200")["logs"]

    def settled(self):
        def check():
            logs = self.logs()
            failures = [log for log in logs if log["status"] == "failed"]
            assert not failures, json.dumps(failures, indent=2)
            return logs and all(log["status"] != "running" for log in logs)
        wait_for("plugin hooks to finish", check)

    def diagnostic(self):
        print(self.log_path.read_text()[-8000:], file=sys.stderr)
        if self.process.poll() is None and self.socket.exists():
            try:
                print(json.dumps(self.run("workspace", "list"), indent=2), file=sys.stderr)
                print(json.dumps(self.logs(), indent=2)[-12000:], file=sys.stderr)
            except Exception as error:
                print("Could not read private logs: {}".format(error), file=sys.stderr)

    def stop(self):
        if self.process.poll() is None:
            try:
                command([self.herdr, "server", "stop"], self.root, self.env)
                self.process.wait(timeout=TIMEOUT)
            except (OSError, subprocess.TimeoutExpired, AssertionError):
                self.process.terminate()
                try:
                    self.process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait(timeout=3)
        self.log_file.close()


def private_environment(root):
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(("HERDR_", "GIT_"))
           and key not in {"TMUX", "TMUX_PANE", "ENV", "BASH_ENV", "ZDOTDIR"}}
    config = root / "config" / "herdr" / "config.toml"
    config.parent.mkdir(parents=True)
    config.write_text(
        'onboarding = false\nconfirm_close = false\n'
        '[terminal]\ndefault_shell = "/bin/sh"\nshell_mode = "non_login"\n'
        '[update]\nversion_check = false\nmanifest_check = false\n'
        '[ui.toast]\ndelivery = "off"\n[ui.sound]\nenabled = false\n'
        '[ui.sidebar.spaces]\n'
        'rows = [["state_icon", "$short_space"], ["$short_branch", "git_status"]]\n'
    )
    (root / "runtime").mkdir(mode=0o700)
    (root / "zdot").mkdir()
    env.update({
        "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state"),
        "XDG_DATA_HOME": str(root / "data"), "XDG_RUNTIME_DIR": str(root / "runtime"),
        "XDG_CACHE_HOME": str(root / "cache"), "HERDR_CONFIG_PATH": str(config),
        "SHELL": "/bin/sh", "ZDOTDIR": str(root / "zdot"),
        "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_TERMINAL_PROMPT": "0",
    })
    return env


def smoke(root, herdr, plugin_dir, servers):
    env = private_environment(root)
    repo = root / "example-repo"
    repo.mkdir()
    git = shutil.which("git")
    assert git, "git is required"

    def run_git(*args, cwd=repo):
        return command([git] + list(args), cwd, env)

    run_git("init", "-b", "main")
    run_git("config", "user.name", "Branch label fixture")
    run_git("config", "user.email", "branch-labels@example.invalid")
    run_git("commit", "--allow-empty", "-m", "Fixture base")
    checkout = root / "folder-name-differs-from-branch"
    branch = "feat/2026-09-10-invoice-validation"
    run_git("worktree", "add", "-b", branch, str(checkout), "main")

    server = Server(root, herdr, env)
    servers.append(server)
    server.ready()
    server.run("plugin", "link", str(plugin_dir))
    parent = server.run("workspace", "create", "--cwd", str(repo), "--no-focus")["workspace"]
    child = server.run("worktree", "open", "--cwd", str(repo), "--path", str(checkout),
                       "--no-focus")["workspace"]
    parent_id, child_id = parent["workspace_id"], child["workspace_id"]
    original_labels = {parent_id: parent["label"], child_id: child["label"]}
    server.tokens(parent_id, "example-repo", "main")
    server.tokens(child_id, "invoice-validation")
    assert run_git("branch", "--show-current", cwd=checkout) == branch
    assert {wid: server.workspace(wid)["label"] for wid in original_labels} == original_labels
    print("PASS: grouped worktrees display the branch suffix without renaming branches or spaces")

    next_branch = "fix/2026-09-10-checkout-transition"
    run_git("checkout", "-b", next_branch, cwd=checkout)
    server.refresh()
    server.tokens(child_id, "checkout-transition")
    assert server.workspace(child_id)["label"] == original_labels[child_id]
    assert run_git("branch", "--show-current", cwd=checkout) == next_branch
    print("PASS: refresh follows branch checkout rather than the checkout directory name")

    manual = "feat/2026-09-10-keep-this-manual-label"
    server.run("workspace", "rename", child_id, manual)
    server.tokens(child_id, manual)
    server.run("workspace", "focus", parent_id)
    server.run("workspace", "focus", child_id)
    server.settled()
    server.tokens(child_id, manual)
    server.saved(child_id, manual)
    server.refresh()
    server.tokens(child_id, manual)
    assert server.workspace(child_id)["label"] == manual
    print("PASS: manual labels, including labels matching the prefix pattern, are preserved")

    config_dir = root / "config" / "herdr" / "plugins" / "config" / PLUGIN_ID
    config_dir.mkdir(parents=True, exist_ok=True)
    (config_dir / "config.json").write_text(json.dumps({
        "pattern": r"^ticket/[0-9]+-(.+)$", "replacement": r"\1",
    }))
    configured_branch = "ticket/123-custom-regex"
    run_git("checkout", "-b", configured_branch)
    server.refresh()
    server.tokens(parent_id, "example-repo", "custom-regex")
    server.tokens(child_id, manual)
    print("PASS: custom regex and replacement change displayed branch metadata")

    server.saved(parent_id)
    server.saved(child_id, manual)
    server.settled()
    server.stop()
    server = Server(root, herdr, env)
    servers.append(server)
    server.ready()
    server.tokens(parent_id, "example-repo", "custom-regex")
    server.tokens(child_id, manual)
    server.settled()
    assert any(log.get("event") == "startup" for log in server.logs())
    assert server.workspace(child_id)["label"] == manual
    assert run_git("branch", "--show-current", cwd=checkout) == next_branch
    assert run_git("branch", "--show-current") == configured_branch
    print("PASS: cold restart rebuilds metadata and preserves manual names and Git branches")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--herdr", default=shutil.which("herdr"))
    args = parser.parse_args()
    if not args.herdr or os.name != "posix":
        print("SKIP: an installed Herdr and Unix sockets are required")
        return 0
    plugin_dir = Path(__file__).resolve().parent.parent
    servers = []
    with tempfile.TemporaryDirectory(prefix="hbl-", dir="/tmp") as directory:
        try:
            smoke(Path(directory).resolve(), str(Path(args.herdr).resolve()), plugin_dir, servers)
        except Exception:
            for server in servers:
                server.diagnostic()
            raise
        finally:
            for server in reversed(servers):
                server.stop()
    return 0


if __name__ == "__main__":
    sys.exit(main())
