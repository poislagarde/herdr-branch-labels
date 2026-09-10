# Branch Labels for Herdr

Show the useful part of a Git branch in the Spaces sidebar. For example,
`feat/2026-09-10-improve-sidebar-labels` becomes `improve-sidebar-labels`.
Customize the pattern and replacement to match your branch convention.

Requires Herdr 0.9.0 or newer, Python 3.9 or newer, and Git on macOS or Linux.

## Install

Install the tagged release from GitHub:

```sh
herdr plugin install poislagarde/herdr-branch-labels --ref v0.1.0
```

In `~/.config/herdr/config.toml`, replace the existing Spaces rows with:

```toml
[ui.sidebar.spaces]
rows = [
  ["state_icon", { token = "$short_space", bold = true }],
  [{ token = "$short_branch", dim = true }, "git_status"],
]
```

Then apply the layout and populate its labels:

```sh
herdr server reload-config
herdr plugin action invoke poislagarde.branch-labels.refresh
```

The plugin reports display-only metadata. It does not rename Git branches or
Herdr workspaces. Custom workspace names remain unchanged. The branch line,
when shown, uses the configured formatting.

## Configure

Find the plugin's configuration directory:

```sh
herdr plugin config-dir poislagarde.branch-labels
```

Create `config.json` in that directory. Both fields below are optional; these
are their defaults:

```json
{
  "pattern": "^[^/]+/[0-9]{4}-[0-9]{2}-[0-9]{2}-(?=.)",
  "replacement": ""
}
```

The pattern uses Python regular-expression syntax. The plugin replaces only
the first match in each branch. An unmatched branch stays unchanged. If a
replacement would produce an empty label, the original branch is retained.

To strip just `feat/` or `fix/`:

```json
{
  "pattern": "^(?:feat|fix)/",
  "replacement": ""
}
```

To turn `feat/ABC-123-improve-sidebar` into `improve-sidebar [ABC-123]`, capture
the issue key and description, then reference them in the replacement:

```json
{
  "pattern": "^[^/]+/([A-Z]+-[0-9]+)-(.+)$",
  "replacement": "\\g<2> [\\g<1>]"
}
```

JSON requires doubled backslashes in replacement references. After changing
`config.json`, run the refresh action again; no config reload is needed.

## Refresh behavior

Labels refresh at server startup and on workspace lifecycle, workspace focus,
and pane focus events. The plugin has no background daemon. After checking out
another branch in the focused pane, change focus or invoke the refresh action.
New spaces initially show their workspace name; formatting can take about five
seconds while Herdr saves the identity hints used to preserve custom names.

Herdr clears metadata after a server restart; the startup hook repopulates it.
Herdr also caps metadata values at 80 characters, and sidebar width still
limits the visible portion of longer labels.

The plugin reads Git state and Herdr's saved version 3 session data to identify
custom workspace names and the directory that determines workspace identity.
It does not write either. When those saved hints are unavailable or cannot be
used safely, it conservatively preserves workspace names from the API.

## Disable

Restore the built-in tokens in `~/.config/herdr/config.toml`:

```toml
[ui.sidebar.spaces]
rows = [
  ["state_icon", "workspace"],
  ["branch", "git_status"],
]
```

Then reload and disable the plugin:

```sh
herdr server reload-config
herdr plugin disable poislagarde.branch-labels
```

## Test

Run from this directory:

```sh
python3 -m unittest discover -s tests
python3 tests/herdr_smoke.py
```

## Local development

Clone the repository and link the checkout:

```sh
git clone https://github.com/poislagarde/herdr-branch-labels.git
cd herdr-branch-labels
herdr plugin link "$(pwd)" --enabled
herdr plugin action invoke poislagarde.branch-labels.refresh
```

Uninstall an existing GitHub-managed installation before linking a local copy
with the same plugin ID. Herdr keeps the plugin's configuration and state.

## License

[MIT](LICENSE) — Copyright (c) 2026 Pablo Ois Lagarde.
