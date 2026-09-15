//! TOML config structs + mapping defaults with user overrides.

use anyhow::Context;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub jira: JiraConfig,
    #[serde(default)]
    pub status_map: HashMap<String, String>,
    #[serde(default)]
    pub priority_map: HashMap<String, String>,
    #[serde(default)]
    pub project_map: HashMap<String, String>,
    #[serde(default)]
    pub projects: HashMap<String, ProjectOverrides>,
    #[serde(default)]
    pub aven: AvenConfig,
}

#[derive(Debug, Default, Deserialize)]
pub struct AvenConfig {
    pub workspace: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ProjectOverrides {
    #[serde(default)]
    pub status_map: HashMap<String, String>,
    #[serde(default)]
    pub priority_map: HashMap<String, String>,
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
// Status/priority chain: [projects.<KEY>.*_map] -> global *_map -> built-in default.
// [projects.<KEY>] key matches Jira project key exactly; value keys case-insensitive.
impl Config {
    fn find(m: &HashMap<String, String>, key: &str) -> Option<String> {
        m.iter()
            .find(|(k, _)| k.to_lowercase() == key)
            .map(|(_, v)| v.clone())
    }

    pub fn map_status(&self, project_key: &str, jira_status: &str) -> String {
        let key = &jira_status.to_lowercase();
        self.projects
            .get(project_key)
            .and_then(|o| Self::find(&o.status_map, key))
            .or_else(|| Self::find(&self.status_map, key))
            .unwrap_or_else(|| default_status(key))
    }

    pub fn map_priority(&self, project_key: &str, jira_priority: &str) -> String {
        let key = &jira_priority.to_lowercase();
        self.projects
            .get(project_key)
            .and_then(|o| Self::find(&o.priority_map, key))
            .or_else(|| Self::find(&self.priority_map, key))
            .unwrap_or_else(|| default_priority(key))
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

// AIDEV-NOTE: XDG base dir spec — default config is $XDG_CONFIG_HOME/jira-aven-sync/config.toml
// (or ~/.config/... when unset), used only when that file exists; else ./config.toml (back-compat).
pub fn resolve_default_config() -> PathBuf {
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    resolve_default_config_at(xdg, home)
}

fn resolve_default_config_at(xdg: Option<PathBuf>, home: Option<PathBuf>) -> PathBuf {
    if let Some(base) = xdg.or_else(|| home.map(|h| h.join(".config"))) {
        let candidate = base.join("jira-aven-sync").join("config.toml");
        if candidate.exists() {
            return candidate;
        }
    }
    PathBuf::from("config.toml")
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
    fn aven_section_parses_workspace() {
        let c = cfg("[jira]\nurl='u'\nemail='e'\njql='j'\n[aven]\nworkspace='salaryhero'\n");
        assert_eq!(c.aven.workspace.as_deref(), Some("salaryhero"));
    }

    #[test]
    fn missing_aven_section_yields_none() {
        let c = cfg("[jira]\nurl='u'\nemail='e'\njql='j'\n");
        assert_eq!(c.aven.workspace, None);
    }

    #[test]
    fn default_status_mapping() {
        let c = cfg("[jira]\nurl='u'\nemail='e'\njql='j'\n");
        assert_eq!(c.map_status("IMP", "In Progress"), "active");
        assert_eq!(c.map_status("IMP", "Done"), "done");
        assert_eq!(c.map_status("IMP", "To Do"), "todo");
        assert_eq!(c.map_status("IMP", "Blocked"), "todo"); // unknown open -> todo
    }

    #[test]
    fn default_priority_mapping() {
        let c = cfg("[jira]\nurl='u'\nemail='e'\njql='j'\n");
        assert_eq!(c.map_priority("IMP", "Highest"), "urgent");
        assert_eq!(c.map_priority("IMP", "Lowest"), "low");
        assert_eq!(c.map_priority("IMP", "Weird"), "medium");
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
        assert_eq!(c.map_status("IMP", "Done"), "canceled");
        assert_eq!(c.map_priority("IMP", "High"), "urgent");
        assert_eq!(c.map_project("IMP"), "x");
    }

    #[test]
    fn case_insensitive_lookup() {
        let c = cfg("[jira]\nurl='u'\nemail='e'\njql='j'\n[status_map]\n'IN PROGRESS'='active'\n");
        assert_eq!(c.map_status("IMP", "in progress"), "active");
    }

    #[test]
    fn per_project_override_wins_over_global() {
        let c = cfg(
            "[jira]\nurl='u'\nemail='e'\njql='j'\n[status_map]\n'Done'='active'\n\
             [projects.IMP.status_map]\n'Done'='canceled'\n",
        );
        assert_eq!(c.map_status("IMP", "Done"), "canceled");
        assert_eq!(c.map_status("OTHER", "Done"), "active");
    }

    #[test]
    fn per_project_falls_back_to_global_then_default() {
        let c = cfg(
            "[jira]\nurl='u'\nemail='e'\njql='j'\n[status_map]\n'Done'='active'\n\
             [projects.IMP.priority_map]\n'High'='low'\n",
        );
        // global status fallback, built-in priority fallback
        assert_eq!(c.map_status("IMP", "Done"), "active");
        assert_eq!(c.map_status("IMP", "In Progress"), "active");
        assert_eq!(c.map_priority("IMP", "High"), "low");
        assert_eq!(c.map_priority("IMP", "Highest"), "urgent");
        // unknown project section -> plain global/default chain
        assert_eq!(c.map_status("ZZZ", "Done"), "active");
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

    fn tmpdir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn default_config_prefers_existing_xdg_path() {
        let xdg = tmpdir("jas-xdg-exists");
        let app = xdg.join("jira-aven-sync");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("config.toml"), "").unwrap();
        let got = resolve_default_config_at(Some(xdg.clone()), None);
        assert_eq!(got, app.join("config.toml"));
        std::fs::remove_dir_all(&xdg).ok();
    }

    #[test]
    fn default_config_uses_home_dot_config_when_xdg_unset() {
        let home = tmpdir("jas-home-config");
        let app = home.join(".config").join("jira-aven-sync");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("config.toml"), "").unwrap();
        let got = resolve_default_config_at(None, Some(home.clone()));
        assert_eq!(got, app.join("config.toml"));
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn default_config_falls_back_to_local_when_xdg_file_missing() {
        let xdg = tmpdir("jas-xdg-missing");
        assert_eq!(
            resolve_default_config_at(Some(xdg.clone()), None),
            std::path::PathBuf::from("config.toml")
        );
        assert_eq!(
            resolve_default_config_at(None, None),
            std::path::PathBuf::from("config.toml")
        );
        std::fs::remove_dir_all(&xdg).ok();
    }

    #[test]
    fn load_errors_on_malformed_toml() {
        let dir = std::env::temp_dir().join("jas-malformed-test.toml");
        std::fs::write(&dir, "not [ valid toml").unwrap();
        let err = load(&dir).unwrap_err();
        assert!(err.to_string().contains(&dir.display().to_string()));
    }
}
