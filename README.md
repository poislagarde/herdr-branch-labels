# Branch Labels for Herdr

Format Git branch labels in the Spaces sidebar with your own regular
expression and replacement. Branch names stay unchanged until you configure
a pattern; the plugin assumes no naming convention.

Requires Herdr 0.9.0 or newer, Rust/Cargo, and Git on macOS or Linux.
Herdr builds the native Rust binary during installation.

## Install

Install the tagged release from GitHub:

```sh
herdr plugin install poislagarde/herdr-branch-labels --ref v0.2.0
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

Create `config.json` in that directory. The default configuration is:

```json
{}
```

Omitting `pattern`, or setting it to `null`, disables formatting. When you
provide a pattern, `replacement` defaults to an empty string. The pattern uses
[fancy-regex syntax](https://docs.rs/fancy-regex/latest/fancy_regex/), including
lookahead and lookbehind. The plugin replaces only the first match
in each branch. An unmatched branch stays unchanged. If a replacement would
produce an empty label, the original branch is retained.

For example, configure this rule to turn `feature/improve-sidebar` into
`improve-sidebar`:

```json
{
  "pattern": "^feature/",
  "replacement": ""
}
```

To turn `ticket/ABC-123-improve-sidebar` into `improve-sidebar [ABC-123]`, capture
the issue key and description, then reference them in the replacement:

```json
{
  "pattern": "^ticket/([A-Z]+-[0-9]+)-(.+)$",
  "replacement": "$2 [$1]"
}
```

Replacements use `$1` for a numbered capture, `${name}` for a named capture,
and `$$` for a literal dollar sign. Use `${1}` before adjoining text to make
the capture boundary explicit. After changing `config.json`, run the refresh
action again; no config reload is needed.

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

Run from this directory. The isolated Herdr smoke test also needs Python 3.9+:

```sh
cargo test --locked
python3 tests/herdr_smoke.py
```

## Local development

Clone the repository and link the checkout:

```sh
git clone https://github.com/poislagarde/herdr-branch-labels.git
cd herdr-branch-labels
cargo build --release --locked
herdr plugin link "$(pwd)" --enabled
herdr plugin action invoke poislagarde.branch-labels.refresh
```

Uninstall an existing GitHub-managed installation before linking a local copy
with the same plugin ID. Herdr keeps the plugin's configuration and state.

## License

[MIT](LICENSE) — Copyright (c) 2026 Pablo Ois Lagarde.
