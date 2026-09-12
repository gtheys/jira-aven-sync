//! aven subprocess adapter: find_by_jira_key, list_synced, add, edit.
//! Shell-out only — never link aven-core (locked in plan).

use std::collections::HashMap;
use std::process::{Command, Stdio};

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;

/// What the sync engine (Phase 4 diff) compares. Kept minimal on purpose.
pub struct AvenTask {
    pub ref_: String,
    pub status: String,
    pub priority: String,
    pub title: String,
    pub description: String,
    pub labels: Vec<String>,
    pub project: String,
    pub jira_status: String,
    pub jira_key: String,
}

/// Fields for a new aven task; description fed via --description-stdin.
pub struct NewTask<'a> {
    pub title: &'a str,
    pub project: &'a str,
    pub status: &'a str,
    pub priority: &'a str,
    pub labels: &'a [String],
    pub jira_key: &'a str,
    pub jira_url: &'a str,
    pub jira_status: &'a str,
    pub description: &'a str,
}

/// Changed fields only; None = leave untouched.
#[derive(Default)]
pub struct EditChanges<'a> {
    pub title: Option<&'a str>,
    pub status: Option<&'a str>,
    pub priority: Option<&'a str>,
    pub description: Option<&'a str>,
    pub add_labels: &'a [String],
    pub remove_labels: &'a [String],
    /// Metadata jira-status update (Jira status name). Emitted as --metadata jira-status=…
    pub jira_status: Option<&'a str>,
}

// AIDEV-NOTE: aven --json shape pinned empirically 2026-09-12 (list/show --json);
// may drift on aven upgrades — adjust these two structs if list parsing fails.
#[derive(Deserialize)]
struct ListItem {
    #[serde(rename = "ref")]
    ref_: String,
    title: String,
    project: String,
    status: String,
    priority: String,
    #[serde(default)]
    labels: Vec<String>,
}

/// Spawn `aven` (plain name — must be on PATH), capture stdout, check exit status.
fn run(args: &[&str]) -> Result<String> {
    let out = Command::new("aven")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                anyhow!(
                    "aven CLI not found on PATH — install prerequisite (cargo install --path <aven repo>)"
                )
            } else {
                anyhow!("failed to spawn aven: {e}")
            }
        })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let snippet: String = stderr.lines().take(5).collect::<Vec<_>>().join("; ");
        bail!("aven {} failed ({}): {}", args.join(" "), out.status, snippet);
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Spawn `aven` with description piped via stdin (--description-stdin).
fn run_with_stdin(args: &[&str], stdin_data: &str) -> Result<()> {
    use std::io::Write;
    let mut child = Command::new("aven")
        .args(args)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                anyhow!(
                    "aven CLI not found on PATH — install prerequisite (cargo install --path <aven repo>)"
                )
            } else {
                anyhow!("failed to spawn aven: {e}")
            }
        })?;
    child
        .stdin
        .take()
        .context("aven stdin not piped")?
        .write_all(stdin_data.as_bytes())
        .context("writing description to aven stdin")?;
    let out = child.wait_with_output().context("waiting for aven")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let snippet: String = stderr.lines().take(5).collect::<Vec<_>>().join("; ");
        bail!("aven {} failed ({}): {}", args.join(" "), out.status, snippet);
    }
    Ok(())
}

/// Existing aven task carrying metadata jira-key=<key>, or None.
/// Unparseable/deleted entries are treated as missing → sync recreates (spec edge case).
// AIDEV-NOTE: aven metadata fields are lazily registered — the field only exists after
// the first task is created with it. Until then `list --metadata jira-key=X` fails with
// `unknown-metadata-field`; treated as "nothing synced yet" (all ADD). First real run
// registers the field via `aven add`, subsequent runs get real lookups.
pub fn find_by_jira_key(key: &str) -> Result<Option<AvenTask>> {
    match run(&["list", "--metadata", &format!("jira-key={key}"), "--json"]) {
        Err(e) if e.to_string().contains("unknown-metadata-field") => Ok(None),
        Err(e) => Err(e),
        Ok(json) => match parse_list(&json)?.into_iter().next() {
            None => Ok(None),
            Some(item) => Ok(Some(enrich(item)?)),
        },
    }
}

/// All aven tasks with jira-key metadata (for the missing-detection pass).
pub fn list_synced() -> Result<Vec<AvenTask>> {
    match run(&["list", "--has-metadata", "jira-key", "--json"]) {
        Err(e) if e.to_string().contains("unknown-metadata-field") => Ok(vec![]),
        Err(e) => Err(e),
        Ok(json) => parse_list(&json)?.into_iter().map(enrich).collect(),
    }
}

pub fn add(input: &NewTask) -> Result<()> {
    let mut args = vec![
        "add".to_string(),
        "--project".into(),
        input.project.to_string(),
        "--status".into(),
        input.status.to_string(),
        "--priority".into(),
        input.priority.to_string(),
    ];
    for l in input.labels {
        args.extend(["--label".into(), l.clone()]);
    }
    args.extend([
        "--metadata".into(),
        format!("jira-key={}", input.jira_key),
        "--metadata".into(),
        format!("jira-url={}", input.jira_url),
        "--metadata".into(),
        format!("jira-status={}", input.jira_status),
        "--description-stdin".into(),
    ]);
    let argrefs: Vec<&str> = args.iter().map(String::as_str).collect();
    // title last (positional)
    let mut argv = argrefs;
    argv.push(input.title);
    run_with_stdin(&argv, input.description)
}

pub fn edit(ref_: &str, changes: &EditChanges) -> Result<()> {
    let mut args: Vec<String> = vec!["edit".into(), ref_.into()];
    if let Some(t) = changes.title {
        args.extend(["--title".into(), t.into()]);
    }
    if let Some(s) = changes.status {
        args.extend(["--status".into(), s.into()]);
    }
    if let Some(p) = changes.priority {
        args.extend(["--priority".into(), p.into()]);
    }
    for l in changes.add_labels {
        args.extend(["--label".into(), l.into()]);
    }
    for l in changes.remove_labels {
        args.extend(["--remove-label".into(), l.into()]);
    }
    if let Some(s) = changes.jira_status {
        args.extend(["--metadata".into(), format!("jira-status={s}")]);
    }
    let needs_stdin = changes.description.is_some();
    if needs_stdin {
        args.push("--description-stdin".into());
    }
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    if let Some(d) = changes.description {
        run_with_stdin(&argv, d)
    } else {
        run(&argv).map(|_| ())
    }
}

/// Create a label. Idempotent in aven: already-existing label exits 0 (pinned 2026-09-12
/// with a throwaway HOME — second `label create` prints created-label again, exit 0).
pub fn create_label(name: &str) -> Result<()> {
    run(&["label", "create", name]).map(|_| ())
}

/// Create a project. Idempotent like label create (pinned 2026-09-12: second
/// `project create` exits 0). Needed because `add --project X` errors with
/// "near-match project" when X doesn't exist yet.
pub fn create_project(key: &str) -> Result<()> {
    run(&["project", "create", key]).map(|_| ())
}

fn parse_list(json: &str) -> Result<Vec<ListItem>> {
    let v: Value =
        serde_json::from_str(json).with_context(|| format!("aven list --json not JSON: {json}"))?;
    Ok(serde_json::from_value(v)?)
}

/// `aven show <ref> --full` text format: description<<EOF ... EOF and
/// metadata field_id=... key=K\nvalue<<EOF ... EOF blocks. Fill in what list --json lacks.
// ponytail: naive line parser; a description containing a bare `EOF` line truncates it —
// upgrade to a JSON show output if aven ever gains `show --json --metadata`.
fn enrich(item: ListItem) -> Result<AvenTask> {
    let full = run(&["show", &item.ref_, "--full"])?;
    let (description, metadata) = parse_full(&full);
    Ok(AvenTask {
        ref_: item.ref_,
        status: item.status,
        priority: item.priority,
        title: item.title,
        description,
        labels: item.labels,
        project: item.project,
        jira_status: metadata.get("jira-status").cloned().unwrap_or_default(),
        jira_key: metadata.get("jira-key").cloned().unwrap_or_default(),
    })
}

fn parse_full(text: &str) -> (String, HashMap<String, String>) {
    let mut description = String::new();
    let mut metadata = HashMap::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if line.starts_with("description<<EOF") {
            let mut buf = Vec::new();
            for l in lines.by_ref() {
                if l == "EOF" {
                    break;
                }
                buf.push(l);
            }
            description = buf.join("\n");
        } else if let Some(k) = line
            .strip_prefix("metadata ")
            .and_then(|s| s.split("key=").nth(1))
        {
            // value block: value<<EOF ... EOF
            let mut buf = Vec::new();
            let mut started = false;
            for l in lines.by_ref() {
                if l == "value<<EOF" {
                    started = true;
                    continue;
                }
                if started {
                    if l == "EOF" {
                        break;
                    }
                    buf.push(l);
                }
            }
            metadata.insert(k.to_string(), buf.join("\n"));
        }
    }
    (description, metadata)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Canned from real `aven list --json` (throwaway HOME, 2026-09-12).
    const LIST_ONE: &str = r#"[
  {
    "ref": "TST-1GCZ",
    "id": "1GCZRMDFB43GC894",
    "title": "Test task",
    "project": "test",
    "status": "todo",
    "priority": "high",
    "labels": ["jira"],
    "deleted": false,
    "is_epic": false,
    "epic_parent": null,
    "epic_children": [],
    "has_conflict": false,
    "blocked_by": 0,
    "blocks": 0,
    "available_at": "",
    "due_on": "",
    "recurrence": null,
    "recurrence_group": null,
    "created_at": "2026-09-12T03:57:00Z",
    "updated_at": "2026-09-12T03:57:00Z"
  }
]"#;

    #[test]
    fn list_one_hit_parses() {
        let items = parse_list(LIST_ONE).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].ref_, "TST-1GCZ");
        assert_eq!(items[0].status, "todo");
        assert_eq!(items[0].priority, "high");
        assert_eq!(items[0].labels, vec!["jira"]);
    }

    #[test]
    fn list_zero_hits() {
        let items = parse_list("[]").unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn list_malformed_is_error() {
        assert!(parse_list("not json at all").is_err());
    }

    // Canned from real `aven show TST-1GCZ --full`.
    const FULL: &str = "\
TST-1GCZ status=todo priority=high labels=jira title=\"Test task\"
id=1GCZRMDFB43GC894
project=test prefix=TST
created=2026-09-12T03:57:00Z updated=2026-09-12T03:57:00Z
description<<EOF
Desc one

Para two
EOF
metadata field_id=VV1XTWT2YQ5C0D6Y key=jira-key
value<<EOF
TEST-1
EOF
metadata field_id=Y8YJY28FNAE6H4NS key=jira-status
value<<EOF
To Do
EOF
metadata field_id=8FBWCCKD5CD66X34 key=jira-url
value<<EOF
https://ex.atlassian.net/browse/TEST-1
EOF
depends_on open=0 total=0
blocks open=0 total=0
Related total=0
";

    #[test]
    fn parse_full_extracts_description_and_metadata() {
        let (desc, meta) = parse_full(FULL);
        assert_eq!(desc, "Desc one\n\nPara two");
        assert_eq!(meta.get("jira-key").unwrap(), "TEST-1");
        assert_eq!(meta.get("jira-status").unwrap(), "To Do");
        assert_eq!(meta.get("jira-url").unwrap(), "https://ex.atlassian.net/browse/TEST-1");
    }

    #[test]
    fn parse_full_empty_description() {
        let (desc, meta) = parse_full("ref=abc status=todo\n");
        assert_eq!(desc, "");
        assert!(meta.is_empty());
    }
}
