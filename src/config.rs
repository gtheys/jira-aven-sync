//! TOML config structs + mapping defaults with user overrides.

use anyhow::Context;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub jira: JiraConfig,
    #[serde(default)]
    pub status_map: HashMap<String, String>,
    #[serde(default)]
    pub priority_map: HashMap<String, String>,
    #[serde(default)]
    pub project_map: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct JiraConfig {
    pub url: String,
    pub email: String,
    pub jql: String,
    #[serde(default)]
    pub missing: MissingBehavior,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MissingBehavior {
    #[default]
    Ignore,
    Done,
}

// AIDEV-NOTE: lookup is case-insensitive over merged maps (defaults + user TOML, user wins).
// Defaults live here so unmapped Jira values still produce valid aven values.
impl Config {
    pub fn map_status(&self, jira_status: &str) -> String {
        let key = jira_status.to_lowercase();
        self.status_map
            .iter()
            .find(|(k, _)| k.to_lowercase() == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| default_status(&key))
    }

    pub fn map_priority(&self, jira_priority: &str) -> String {
        let key = jira_priority.to_lowercase();
        self.priority_map
            .iter()
            .find(|(k, _)| k.to_lowercase() == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| default_priority(&key))
    }

    pub fn map_project(&self, jira_project: &str) -> String {
        self.project_map
            .get(jira_project)
            .cloned()
            .unwrap_or_else(|| jira_project.to_string())
    }
}

fn default_status(key: &str) -> String {
    match key {
        "to do" | "open" | "new" | "backlog" => "todo",
        "in progress" | "in review" => "active",
        "done" | "resolved" | "closed" => "done",
        "cancelled" | "canceled" => "canceled",
        // unmapped open Jira statuses fall through to todo
        _ => "todo",
    }
    .into()
}

fn default_priority(key: &str) -> String {
    match key {
        "highest" | "urgent" => "urgent",
        "high" => "high",
        "medium" => "medium",
        "low" | "lowest" => "low",
        _ => "medium",
    }
    .into()
}

pub fn load(path: &Path) -> anyhow::Result<Config> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config file {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("failed to parse config file {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(toml_str: &str) -> Config {
        toml::from_str(toml_str).unwrap()
    }

    #[test]
    fn default_status_mapping() {
        let c = cfg("[jira]\nurl='u'\nemail='e'\njql='j'\n");
        assert_eq!(c.map_status("In Progress"), "active");
        assert_eq!(c.map_status("Done"), "done");
        assert_eq!(c.map_status("To Do"), "todo");
        assert_eq!(c.map_status("Blocked"), "todo"); // unknown open -> todo
    }

    #[test]
    fn default_priority_mapping() {
        let c = cfg("[jira]\nurl='u'\nemail='e'\njql='j'\n");
        assert_eq!(c.map_priority("Highest"), "urgent");
        assert_eq!(c.map_priority("Lowest"), "low");
        assert_eq!(c.map_priority("Weird"), "medium");
    }

    #[test]
    fn project_identity_fallback() {
        let c = cfg("[jira]\nurl='u'\nemail='e'\njql='j'\n");
        assert_eq!(c.map_project("IMP"), "IMP");
    }

    #[test]
    fn user_override_wins() {
        let c = cfg(
            "[jira]\nurl='u'\nemail='e'\njql='j'\n[status_map]\n'Done'='canceled'\n[priority_map]\n'High'='urgent'\n[project_map]\n'IMP'='x'\n",
        );
        assert_eq!(c.map_status("Done"), "canceled");
        assert_eq!(c.map_priority("High"), "urgent");
        assert_eq!(c.map_project("IMP"), "x");
    }

    #[test]
    fn case_insensitive_lookup() {
        let c = cfg("[jira]\nurl='u'\nemail='e'\njql='j'\n[status_map]\n'IN PROGRESS'='active'\n");
        assert_eq!(c.map_status("in progress"), "active");
    }

    #[test]
    fn missing_parses_and_defaults() {
        let c = cfg("[jira]\nurl='u'\nemail='e'\njql='j'\nmissing='done'\n");
        assert_eq!(c.jira.missing, MissingBehavior::Done);
        let d = cfg("[jira]\nurl='u'\nemail='e'\njql='j'\n");
        assert_eq!(d.jira.missing, MissingBehavior::Ignore);
    }

    #[test]
    fn load_errors_on_missing_file() {
        let err = load(Path::new("/nonexistent/config.toml")).unwrap_err();
        assert!(err.to_string().contains("/nonexistent/config.toml"));
    }

    #[test]
    fn load_errors_on_malformed_toml() {
        let dir = std::env::temp_dir().join("jas-malformed-test.toml");
        std::fs::write(&dir, "not [ valid toml").unwrap();
        let err = load(&dir).unwrap_err();
        assert!(err.to_string().contains(&dir.display().to_string()));
    }
}
