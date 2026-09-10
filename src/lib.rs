pub mod formatter;
pub mod runtime;

use formatter::Formatter;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const SOURCE: &str = "plugin:poislagarde.branch-labels";

pub fn saved_workspaces(socket_path: &Path) -> BTreeMap<String, Value> {
    let read = || -> Option<BTreeMap<String, Value>> {
        let data = std::fs::read_to_string(socket_path.with_file_name("session.json")).ok()?;
        let snapshot: Value = serde_json::from_str(&data).ok()?;
        if snapshot.get("version")?.as_f64()? != 3.0 {
            return None;
        }
        Some(
            snapshot
                .get("workspaces")?
                .as_array()?
                .iter()
                .filter_map(|item| Some((item.get("id")?.as_str()?.to_owned(), item.clone())))
                .collect(),
        )
    };
    read().unwrap_or_default()
}

pub fn current_hint(workspace: &Value, hint: Option<&Value>) -> bool {
    let Some(hint) = hint else { return false };
    let Some(custom_name) = hint.get("custom_name") else {
        return false;
    };
    let name = if custom_name.is_null() {
        let Some(cwd) = hint.get("identity_cwd").and_then(Value::as_str) else {
            return false;
        };
        if !Path::new(cwd).is_absolute() {
            return false;
        }
        // Ignore redundant separators and "." while retaining "..".
        cwd.split('/')
            .rfind(|part| !part.is_empty() && *part != ".")
            .unwrap_or(cwd)
    } else {
        let Some(name) = custom_name.as_str() else {
            return false;
        };
        name
    };
    workspace.get("label").and_then(Value::as_str) == Some(name)
}

pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(v) => v.as_f64().is_some_and(|v| v != 0.0),
        Value::String(v) => !v.is_empty(),
        Value::Array(v) => !v.is_empty(),
        Value::Object(v) => !v.is_empty(),
    }
}

pub fn text_field<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string field {key}"))
}

pub fn indented_workspaces(workspaces: &[Value]) -> Result<BTreeSet<String>, String> {
    let mut groups: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
    for workspace in workspaces {
        if let Some(key) = workspace
            .get("worktree")
            .and_then(|w| w.get("repo_key"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            groups.entry(key).or_default().push(workspace);
        }
    }
    let mut indented = BTreeSet::new();
    for members in groups.values() {
        let mut parent = None;
        for (index, workspace) in members.iter().enumerate() {
            let linked = workspace
                .get("worktree")
                .and_then(|w| w.get("is_linked_worktree"))
                .ok_or("missing is_linked_worktree")?;
            if !truthy(linked) {
                parent = Some(index);
                break;
            }
        }
        if let Some(parent) = parent.filter(|_| members.len() > 1) {
            for (index, workspace) in members.iter().enumerate() {
                if index != parent {
                    indented.insert(text_field(workspace, "workspace_id")?.to_owned());
                }
            }
        }
    }
    Ok(indented)
}

pub fn token_text(value: Option<&str>) -> Option<String> {
    let value = value?;
    let visible: String = value
        .chars()
        .filter(|c| *c >= ' ' && !('\u{7f}'..='\u{9f}').contains(c))
        .collect();
    let normalized = visible.split_whitespace().collect::<Vec<_>>().join(" ");
    let text: String = normalized.chars().take(80).collect();
    (!text.is_empty()).then_some(text)
}

pub fn labels(
    workspace: &Value,
    hint: Option<&Value>,
    branch: Option<&str>,
    indented: bool,
    formatter: &Formatter,
    renamed: bool,
) -> Result<Value, String> {
    let mut name = text_field(workspace, "label")?.to_owned();
    let branch = branch.filter(|branch| !branch.is_empty());
    let automatic = hint
        .and_then(|h| h.get("custom_name"))
        .is_some_and(Value::is_null);
    if indented && automatic && !renamed {
        if let Some(branch) = branch {
            name = formatter.format(branch)?;
        }
    }
    let short_branch = if !indented {
        branch.map(|b| formatter.format(b)).transpose()?
    } else {
        None
    };
    Ok(json!({
        "short_space": token_text(Some(&name)),
        "short_branch": token_text(short_branch.as_deref()),
    }))
}
