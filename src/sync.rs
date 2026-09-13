//! Per-issue decision: ADD / UPDATE / SKIP / MISSING.

use std::collections::HashSet;

use crate::adf;
use crate::aven::{self, AvenTask, EditChanges, NewTask};
use crate::config::{Config, MissingBehavior};
use crate::jira::JiraIssue;
use anyhow::Result;

/// Fully mapped Jira issue, ready for aven. Pure data — tests build these by hand.
pub struct Mapped {
    pub key: String,
    pub title: String, // "[KEY] summary"
    pub project: String,
    pub status: String,
    pub priority: String,
    pub labels: Vec<String>, // jira labels ∪ {"jira"}, deduped
    pub jira_url: String,
    pub jira_status: String, // raw Jira status name (metadata only)
    pub description: String,
}

/// One drifted field; carries the values `aven edit` needs.
#[derive(Debug, PartialEq, Eq)]
pub enum FieldChange {
    Title(String),
    Status(String),
    Priority(String),
    Description(String),
    Labels {
        add: Vec<String>,
        remove: Vec<String>,
    },
    JiraStatus(String),
}

impl FieldChange {
    /// Name for the `UPDATE REF (a, b)` line.
    fn name(&self) -> &'static str {
        match self {
            FieldChange::Title(_) => "title",
            FieldChange::Status(_) => "status",
            FieldChange::Priority(_) => "priority",
            FieldChange::Description(_) => "description",
            FieldChange::Labels { .. } => "labels",
            FieldChange::JiraStatus(_) => "jira-status",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Add,
    Update {
        ref_: String,
        changes: Vec<FieldChange>,
    },
    Skip,
}

// AIDEV-NOTE: Jira-wins (locked bugwarrior semantics) — any drift on an existing
// task is overwritten here, including description and labels. Local aven edits to
// synced fields do NOT survive a sync run. Only `note` appends are safe.
pub fn decide(mapped: &Mapped, existing: Option<&AvenTask>) -> Action {
    let Some(t) = existing else {
        return Action::Add;
    };
    let mut changes = Vec::new();
    if t.title != mapped.title {
        changes.push(FieldChange::Title(mapped.title.clone()));
    }
    if t.status != mapped.status {
        changes.push(FieldChange::Status(mapped.status.clone()));
    }
    if t.priority != mapped.priority {
        changes.push(FieldChange::Priority(mapped.priority.clone()));
    }
    if t.description != mapped.description {
        changes.push(FieldChange::Description(mapped.description.clone()));
    }
    if t.jira_status != mapped.jira_status {
        changes.push(FieldChange::JiraStatus(mapped.jira_status.clone()));
    }
    let mut labels = mapped.labels.clone();
    labels.sort();
    let mut have = t.labels.clone();
    have.sort();
    if labels != have {
        changes.push(FieldChange::Labels {
            add: labels
                .iter()
                .filter(|l| !have.contains(l))
                .cloned()
                .collect(),
            remove: have
                .iter()
                .filter(|l| !labels.contains(l))
                .cloned()
                .collect(),
        });
    }
    if changes.is_empty() {
        Action::Skip
    } else {
        Action::Update {
            ref_: t.ref_.clone(),
            changes,
        }
    }
}

/// Config mapping: Jira issue -> aven-ready Mapped. Pure.
pub fn map(cfg: &Config, issue: &JiraIssue) -> Mapped {
    let mut labels = issue.labels.clone();
    labels.push("jira".into());
    labels.sort();
    labels.dedup();
    Mapped {
        key: issue.key.clone(),
        title: format!("[{}] {}", issue.key, issue.summary),
        project: cfg.map_project(&issue.project_key),
        status: cfg.map_status(&issue.project_key, &issue.status),
        priority: cfg.map_priority(&issue.project_key, issue.priority.as_deref().unwrap_or("medium")),
        labels,
        jira_url: issue.url.clone(),
        jira_status: issue.status.clone(),
        description: issue
            .description
            .as_ref()
            .map(adf::flatten)
            .unwrap_or_default(),
    }
}

// AIDEV-NOTE: missing-detection semantics — Jira is the source of truth. Any aven
// task carrying jira-key metadata whose key left the JQL result set is "missing".
// Default behavior is `ignore` (no surprise mutations); `missing = "done"` in config
// closes the aven task. aven-side tasks are never deleted here.
pub fn compute_missing<'a>(
    synced: &'a [AvenTask],
    result_keys: &HashSet<&str>,
) -> Vec<&'a AvenTask> {
    synced
        .iter()
        .filter(|t| !result_keys.contains(t.jira_key.as_str()))
        .collect()
}

/// Pure decision for the missing pass: which refs to close, if behavior is `done`.
pub fn missing_actions(behavior: MissingBehavior, missing: &[&AvenTask]) -> Vec<(String, String)> {
    match behavior {
        MissingBehavior::Ignore => vec![],
        MissingBehavior::Done => missing
            .iter()
            .map(|t| (t.jira_key.clone(), t.ref_.clone()))
            .collect(),
    }
}

pub fn format_summary(added: u32, updated: u32, skipped: u32, missing_done: u32) -> String {
    let mut s = format!("{added} added, {updated} updated, {skipped} skipped");
    if missing_done > 0 {
        s.push_str(&format!(", {missing_done} missing-done"));
    }
    s
}

/// Ensure every label we will use exists (aven requires labels to pre-exist on add).
/// Covers `jira` plus all labels carried by the JQL result set.
/// aven's `label create` is idempotent (exit 0, "already exists" is not an error).
fn ensure_labels(cfg: &Config, issues: &[JiraIssue]) -> Result<()> {
    let mut labels: Vec<&str> = issues
        .iter()
        .flat_map(|i| i.labels.iter().map(String::as_str))
        .collect();
    labels.push("jira");
    labels.sort_unstable();
    labels.dedup();
    for label in labels {
        aven::create_label(cfg.aven.workspace.as_deref(), label)?;
    }
    // Projects: aven add --project X fails with "near-match project" if X is unknown.
    let mut projects: Vec<String> = issues
        .iter()
        .map(|i| cfg.map_project(&i.project_key))
        .collect();
    projects.sort_unstable();
    projects.dedup();
    for project in projects {
        aven::create_project(cfg.aven.workspace.as_deref(), &project)?;
    }
    Ok(())
}

pub fn run(cfg: &Config, issues: Vec<JiraIssue>, dry_run: bool) -> Result<()> {
    if !dry_run {
        ensure_labels(cfg, &issues)?;
    }
    let (mut added, mut updated, mut skipped) = (0u32, 0u32, 0u32);
    for issue in &issues {
        let mapped = map(cfg, issue);
        let existing = aven::find_by_jira_key(cfg.aven.workspace.as_deref(), &mapped.key)?;
        match decide(&mapped, existing.as_ref()) {
            Action::Add => {
                added += 1;
                println!("ADD {}", mapped.key);
                if !dry_run {
                    aven::add(cfg.aven.workspace.as_deref(), &NewTask {
                        title: &mapped.title,
                        project: &mapped.project,
                        status: &mapped.status,
                        priority: &mapped.priority,
                        labels: &mapped.labels,
                        jira_key: &mapped.key,
                        jira_url: &mapped.jira_url,
                        jira_status: &mapped.jira_status,
                        description: &mapped.description,
                    })?;
                }
            }
            Action::Update { ref_, changes } => {
                updated += 1;
                let names: Vec<&str> = changes.iter().map(|c| c.name()).collect();
                println!("UPDATE {} ({})", mapped.key, names.join(", "));
                if !dry_run {
                    let mut edit = EditChanges::default();
                    for c in &changes {
                        match c {
                            FieldChange::Title(v) => edit.title = Some(v),
                            FieldChange::Status(v) => edit.status = Some(v),
                            FieldChange::Priority(v) => edit.priority = Some(v),
                            FieldChange::Description(v) => edit.description = Some(v),
                            FieldChange::JiraStatus(v) => edit.jira_status = Some(v),
                            FieldChange::Labels { add, remove } => {
                                edit.add_labels = add;
                                edit.remove_labels = remove;
                            }
                        }
                    }
                    aven::edit(cfg.aven.workspace.as_deref(), &ref_, &edit)?;
                }
            }
            Action::Skip => {
                skipped += 1;
                println!("SKIP {}", mapped.key);
            }
        }
    }
    let mut missing_done = 0u32;
    let keys: HashSet<&str> = issues.iter().map(|i| i.key.as_str()).collect();
    if cfg.jira.missing == MissingBehavior::Done {
        let synced = aven::list_synced(cfg.aven.workspace.as_deref())?;
        let missing = compute_missing(&synced, &keys);
        for (key, ref_) in missing_actions(cfg.jira.missing, &missing) {
            println!("MISSING→DONE {key} {ref_}");
            if !dry_run {
                aven::edit(
                    cfg.aven.workspace.as_deref(),
                    &ref_,
                    &EditChanges {
                        status: Some("done"),
                        ..Default::default()
                    },
                )?;
                missing_done += 1;
            }
        }
    }
    println!("{}", format_summary(added, updated, skipped, missing_done));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapped() -> Mapped {
        Mapped {
            key: "IMP-1".into(),
            title: "[IMP-1] Fix bug".into(),
            project: "improvements".into(),
            status: "active".into(),
            priority: "high".into(),
            labels: vec!["backend".into(), "jira".into()],
            jira_url: "https://ex.atlassian.net/browse/IMP-1".into(),
            jira_status: "In Progress".into(),
            description: "Para one\n\nPara two".into(),
        }
    }

    fn task() -> AvenTask {
        AvenTask {
            ref_: "TST-1GCZ".into(),
            status: "active".into(),
            priority: "high".into(),
            title: "[IMP-1] Fix bug".into(),
            description: "Para one\n\nPara two".into(),
            labels: vec!["backend".into(), "jira".into()],
            jira_status: "In Progress".into(),
            jira_key: "IMP-1".into(),
        }
    }

    #[test]
    fn new_issue_adds() {
        assert_eq!(decide(&mapped(), None), Action::Add);
    }

    #[test]
    fn identical_skips() {
        assert_eq!(decide(&mapped(), Some(&task())), Action::Skip);
    }

    #[test]
    fn title_drift_names_title() {
        let mut t = task();
        t.title = "renamed locally".into();
        assert_eq!(
            decide(&mapped(), Some(&t)),
            Action::Update {
                ref_: "TST-1GCZ".into(),
                changes: vec![FieldChange::Title("[IMP-1] Fix bug".into())],
            }
        );
    }

    #[test]
    fn status_drift_names_status() {
        let mut t = task();
        t.status = "todo".into();
        assert_eq!(
            decide(&mapped(), Some(&t)),
            Action::Update {
                ref_: "TST-1GCZ".into(),
                changes: vec![FieldChange::Status("active".into())],
            }
        );
    }

    #[test]
    fn priority_drift_names_priority() {
        let mut t = task();
        t.priority = "medium".into();
        assert_eq!(
            decide(&mapped(), Some(&t)),
            Action::Update {
                ref_: "TST-1GCZ".into(),
                changes: vec![FieldChange::Priority("high".into())],
            }
        );
    }

    #[test]
    fn description_drift_names_description() {
        let mut t = task();
        t.description = "local notes".into();
        assert_eq!(
            decide(&mapped(), Some(&t)),
            Action::Update {
                ref_: "TST-1GCZ".into(),
                changes: vec![FieldChange::Description("Para one\n\nPara two".into())],
            }
        );
    }

    #[test]
    fn jira_status_metadata_drift_names_jira_status() {
        let mut t = task();
        t.jira_status = "To Do".into();
        assert_eq!(
            decide(&mapped(), Some(&t)),
            Action::Update {
                ref_: "TST-1GCZ".into(),
                changes: vec![FieldChange::JiraStatus("In Progress".into())],
            }
        );
    }

    #[test]
    fn missing_label_and_extra_label_both_count() {
        let mut t = task();
        t.labels = vec!["jira".into()]; // missing "backend"
        match decide(&mapped(), Some(&t)) {
            Action::Update { changes, .. } => assert_eq!(
                changes,
                vec![FieldChange::Labels {
                    add: vec!["backend".into()],
                    remove: vec![],
                }]
            ),
            a => panic!("expected Update, got {a:?}"),
        }
        let mut t2 = task();
        t2.labels = vec!["backend".into(), "jira".into(), "local-only".into()];
        match decide(&mapped(), Some(&t2)) {
            Action::Update { changes, .. } => assert_eq!(
                changes,
                vec![FieldChange::Labels {
                    add: vec![],
                    remove: vec!["local-only".into()],
                }]
            ),
            a => panic!("expected Update, got {a:?}"),
        }
    }

    #[test]
    fn label_order_irrelevant() {
        let mut t = task();
        t.labels = vec!["jira".into(), "backend".into()];
        assert_eq!(decide(&mapped(), Some(&t)), Action::Skip);
    }

    #[test]
    fn multi_field_drift_names_all() {
        let mut t = task();
        t.title = "x".into();
        t.status = "todo".into();
        t.priority = "low".into();
        let action = decide(&mapped(), Some(&t));
        let Action::Update { changes, .. } = action else {
            panic!("expected Update");
        };
        let names: Vec<&str> = changes.iter().map(|c| c.name()).collect();
        assert_eq!(names, vec!["title", "status", "priority"]);
    }

    #[test]
    fn terminal_jira_status_maps_to_done_in_action() {
        let mut m = mapped();
        m.jira_status = "Done".into();
        m.status = "done".into();
        let mut t = task();
        t.status = "active".into();
        t.jira_status = "In Progress".into();
        let action = decide(&m, Some(&t));
        assert!(matches!(
            action,
            Action::Update { changes, .. }
                if changes.contains(&FieldChange::Status("done".into()))
                    && changes.contains(&FieldChange::JiraStatus("Done".into()))
        ));
    }

    fn cfg() -> Config {
        toml::from_str("[jira]\nurl='u'\nemail='e'\njql='j'\n").unwrap()
    }

    fn issue() -> JiraIssue {
        JiraIssue {
            key: "IMP-9".into(),
            summary: "Do thing".into(),
            status: "In Progress".into(),
            priority: Some("Highest".into()),
            project_key: "IMP".into(),
            labels: vec!["backend".into()],
            description: Some(serde_json::json!({
                "type": "doc",
                "content": [
                    {"type": "paragraph", "content": [{"type": "text", "text": "hello"}]}
                ]
            })),
            url: "https://ex.atlassian.net/browse/IMP-9".into(),
        }
    }

    #[test]
    fn map_applies_config_and_adds_jira_label() {
        let m = map(&cfg(), &issue());
        assert_eq!(m.title, "[IMP-9] Do thing");
        assert_eq!(m.status, "active"); // default map
        assert_eq!(m.priority, "urgent"); // default map: Highest -> urgent
        assert_eq!(m.project, "IMP"); // identity fallback
        assert_eq!(m.labels, vec!["backend", "jira"]);
        assert_eq!(m.jira_status, "In Progress");
        assert_eq!(m.description, "hello");
    }

    #[test]
    fn map_null_priority_defaults_medium() {
        let mut i = issue();
        i.priority = None;
        assert_eq!(map(&cfg(), &i).priority, "medium");
    }

    #[test]
    fn missing_set_keys_not_in_results() {
        let mut t1 = task();
        t1.jira_key = "IMP-1".into();
        let mut t2 = task();
        t2.jira_key = "IMP-9".into();
        t2.ref_ = "TST-XYZ".into();
        let synced = vec![t1, t2];
        let keys: HashSet<&str> = ["IMP-1", "IMP-2"].into_iter().collect();
        let missing = compute_missing(&synced, &keys);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].jira_key, "IMP-9");
        assert_eq!(missing[0].ref_, "TST-XYZ");
    }

    #[test]
    fn missing_ignore_yields_no_actions() {
        let t = task();
        assert!(missing_actions(MissingBehavior::Ignore, &[&t]).is_empty());
    }

    #[test]
    fn missing_done_yields_key_ref_pairs() {
        let t = task();
        assert_eq!(
            missing_actions(MissingBehavior::Done, &[&t]),
            vec![("IMP-1".into(), "TST-1GCZ".into())]
        );
    }

    #[test]
    fn summary_omits_zero_missing_done() {
        assert_eq!(format_summary(1, 2, 3, 0), "1 added, 2 updated, 3 skipped");
        assert_eq!(
            format_summary(1, 0, 0, 4),
            "1 added, 0 updated, 0 skipped, 4 missing-done"
        );
    }

    #[test]
    fn map_dedupes_jira_label() {
        let mut i = issue();
        i.labels = vec!["jira".into()];
        assert_eq!(map(&cfg(), &i).labels, vec!["jira"]);
    }
}
