//! Blocking Jira client: search(JQL) -> Vec<JiraIssue> (pagination).

use anyhow::{anyhow, bail, Context};
use serde::Deserialize;
use serde_json::json;

use crate::config::JiraConfig;

const FIELDS: [&str; 6] = [
    "summary",
    "status",
    "priority",
    "project",
    "labels",
    "description",
];
const MAX_RESULTS: u32 = 100;

pub struct JiraIssue {
    pub key: String,
    pub summary: String,
    pub status: String,
    pub priority: Option<String>, // Jira priority may be null -> unmapped, config defaults medium
    pub project_key: String,
    pub labels: Vec<String>,
    pub description: Option<serde_json::Value>, // raw ADF — adf.rs flattens it
    pub url: String,
}

// AIDEV-NOTE: REST v3 /search/jql pagination shape is the spec's top risk —
// token-based loop kept small and isolated here; if the endpoint differs, fix in this file only.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    issues: Vec<Issue>,
    #[serde(default)]
    next_page_token: Option<String>,
    #[serde(default)]
    is_last: bool,
}

#[derive(Deserialize)]
struct Issue {
    key: String,
    fields: Fields,
}

#[derive(Deserialize)]
struct Fields {
    summary: String,
    status: Option<Status>,
    priority: Option<Named>,
    project: Option<Project>,
    #[serde(default)]
    labels: Vec<String>,
    description: Option<serde_json::Value>,
}

// AIDEV-NOTE: project key != name (e.g. key "DEVOPS", name "DevOps"); project_key
// feeds project_map lookup + identity fallback, so it MUST be the key. Name is the
// display string. Fall back to name only if key absent (defensive; API always sends it).
#[derive(Deserialize)]
struct Project {
    key: Option<String>,
    name: Option<String>,
}

#[derive(Deserialize)]
struct Status {
    name: Option<String>,
}

#[derive(Deserialize)]
struct Named {
    name: Option<String>,
}

/// Pure pagination decision: Some(next token) if another request is needed.
fn next_token(page: &Page) -> Option<String> {
    if page.is_last {
        None
    } else {
        page.next_page_token.clone()
    }
}

pub fn search(cfg: &JiraConfig) -> anyhow::Result<Vec<JiraIssue>> {
    let token = std::env::var("JIRA_API_TOKEN").map_err(|_| {
        anyhow!("JIRA_API_TOKEN not set — check JIRA_API_TOKEN env var / email in config")
    })?;
    let client = reqwest::blocking::Client::new();
    let endpoint = format!("{}/rest/api/3/search/jql", cfg.url.trim_end_matches('/'));

    let mut issues = Vec::new();
    let mut page_token: Option<String> = None;
    loop {
        let mut body = json!({ "jql": cfg.jql, "maxResults": MAX_RESULTS, "fields": FIELDS });
        if let Some(t) = &page_token {
            body["nextPageToken"] = json!(t);
        }

        let resp = client
            .post(&endpoint)
            .basic_auth(cfg.email.clone(), Some(token.clone()))
            .json(&body)
            .send()
            .context("request to Jira failed (network error)")?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            bail!("Jira auth failed ({status}) — check JIRA_API_TOKEN / email");
        }
        let bytes = resp.bytes().context("failed to read Jira response body")?;
        if !status.is_success() {
            let snippet = String::from_utf8_lossy(&bytes);
            let snippet = snippet.chars().take(300).collect::<String>();
            bail!("Jira returned {status}: {snippet}");
        }

        let page: Page =
            serde_json::from_slice(&bytes).context("failed to parse Jira search response")?;
        page_token = next_token(&page);
        issues.extend(page.issues.into_iter().map(|i| convert(i, &cfg.url)));
        if page_token.is_none() {
            return Ok(issues);
        }
    }
}

fn convert(i: Issue, base_url: &str) -> JiraIssue {
    JiraIssue {
        url: format!("{}/browse/{}", base_url.trim_end_matches('/'), i.key),
        key: i.key,
        summary: i.fields.summary,
        status: i.fields.status.and_then(|s| s.name).unwrap_or_default(),
        priority: i.fields.priority.and_then(|p| p.name),
        project_key: i
            .fields
            .project
            .and_then(|p| p.key.or(p.name))
            .unwrap_or_default(),
        labels: i.fields.labels,
        description: i.fields.description,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(body: &str) -> anyhow::Result<Vec<JiraIssue>> {
        let page: Page = serde_json::from_str(body)?;
        Ok(page
            .issues
            .into_iter()
            .map(|i| convert(i, "https://example.atlassian.net"))
            .collect())
    }

    #[test]
    fn parses_two_issues_with_null_priority_and_missing_description() {
        let issues = parse(
            r#"{
                "issues": [
                    {
                        "key": "IMP-1",
                        "fields": {
                            "summary": "First bug",
                            "status": {"name": "In Progress"},
                            "priority": null,
                            "project": {"key": "DEVOPS", "name": "DevOps"},
                            "labels": ["jira", "backend"]
                        }
                    },
                    {
                        "key": "IMP-2",
                        "fields": {
                            "summary": "Second bug",
                            "status": {"name": "Done"},
                            "priority": {"name": "High"},
                            "project": {"name": "IMP"},
                            "labels": [],
                            "description": {"type": "doc", "content": []}
                        }
                    }
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].key, "IMP-1");
        assert_eq!(issues[0].summary, "First bug");
        assert_eq!(issues[0].status, "In Progress");
        assert_eq!(issues[0].priority, None);
        // key, not name — even though name differs ("DevOps")
        assert_eq!(issues[0].project_key, "DEVOPS");
        assert_eq!(issues[0].labels, vec!["jira", "backend"]);
        assert_eq!(issues[0].description, None);
        assert_eq!(issues[0].url, "https://example.atlassian.net/browse/IMP-1");
        assert_eq!(issues[1].priority, Some("High".into()));
        assert!(issues[1].description.is_some());
    }

    #[test]
    fn empty_page_yields_empty_vec() {
        let issues = parse(r#"{"issues": [], "isLast": true}"#).unwrap();
        assert!(issues.is_empty());
    }

    #[test]
    fn next_token_continues_on_token_stops_on_is_last() {
        let with_token: Page =
            serde_json::from_str(r#"{"issues": [], "nextPageToken": "abc123"}"#).unwrap();
        assert_eq!(next_token(&with_token).as_deref(), Some("abc123"));

        let last: Page =
            serde_json::from_str(r#"{"issues": [], "nextPageToken": "abc123", "isLast": true}"#)
                .unwrap();
        assert_eq!(next_token(&last), None);

        let no_token: Page = serde_json::from_str(r#"{"issues": []}"#).unwrap();
        assert_eq!(next_token(&no_token), None);
    }

    // Live smoke against real Jira — run explicitly:
    //   JIRA_API_TOKEN=... cargo test live_jira -- --ignored --nocapture
    #[test]
    #[ignore = "live Jira smoke — needs JIRA_API_TOKEN and config.toml"]
    fn live_jira_search() {
        let cfg =
            crate::config::load(std::path::Path::new("config.toml")).expect("config.toml loads");
        let issues = search(&cfg.jira).expect("search succeeds against real Jira");
        println!("fetched {} issues", issues.len());
        for issue in issues.iter().take(5) {
            println!(
                "{} [{} / {}] {}",
                issue.key,
                issue.status,
                issue.priority.as_deref().unwrap_or("<none>"),
                issue.summary
            );
        }
        assert!(
            !issues.is_empty(),
            "expected at least one assigned open issue"
        );
    }
}
