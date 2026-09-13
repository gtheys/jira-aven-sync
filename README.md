# jira-aven-sync

One-way sync of Jira Cloud issues into aven tasks — the aven equivalent of bugwarrior's jira→taskwarrior sync. Manual run, no daemon. Jira state wins: local edits to synced fields (title, status, priority, description, labels) are overwritten on each run.

## Prerequisites

- `aven` CLI on PATH — build from the aven repo: `cargo install --path <aven-repo>`
- A Jira Cloud API token: create at `https://id.atlassian.com/manage-profile/security/api-tokens`
- Rust (stable) to build

## Install

From crates.io:

```
cargo install jira-aven-sync
```

Or straight from git:

```
cargo install --git git@github.com:gtheys/jira-aven-sync.git
```

Or from a local clone: `cargo install --path .` (re-run with `--force` to update).
`cargo build` and `target/debug/jira-aven-sync` works too.

## Configuration

Copy `config.example.toml` to `config.toml` and edit. The API token is **never** in the config file — export it:

```
export JIRA_API_TOKEN="your-token"
```

Full annotated example (same as `config.example.toml`):

```toml
[jira]
url = "https://example.atlassian.net"
email = "you@example.com"
jql = "assignee = currentUser() AND statusCategory != Done ORDER BY updated DESC"
# missing = "ignore"   # or "done"

[status_map]
# "In Progress" = "active"

[priority_map]
# "Highest" = "urgent"

[project_map]
# "IMP" = "improvements"

[aven]
# workspace = "salaryhero"   # optional; default: aven infers workspace from cwd
```

## Usage

```
jira-aven-sync [--dry-run] [--config path]
```

- `--dry-run` — print intended actions, mutate nothing
- `--config <path>` — config file (default `config.toml`)

Output is one line per action plus a summary:

- `ADD IMP-123` — new issue, aven task created
- `UPDATE IMP-456 (status, title)` — existing task edited; changed fields listed
- `SKIP IMP-789` — no drift, task untouched
- `MISSING→DONE IMP-1 TST-XYZ` — task closed because its jira-key left the JQL result set (`missing = "done"` only)

Idempotent: identity is aven metadata `jira-key=<KEY>`, so a second run right after the first does nothing (SKIP-only). Deleting an aven task and re-running recreates it. JQL matching zero issues is a no-op, exit 0.

## Mappings and defaults

Each synced field is resolved independently:

- **status / priority** — lookup order: `[projects.<KEY>.status_map]`/`[projects.<KEY>.priority_map]`
  → global `[status_map]`/`[priority_map]` → built-in defaults below. The first hit wins.
- **project** — `[project_map]` on the Jira project key; unmapped keys are used verbatim.

Matching rules:

- Value keys (Jira status/priority names) match **case-insensitively** — `"In Review"` covers `in review`.
- Project keys (`<KEY>` in `[projects.<KEY>]` and `[project_map]`) match **exactly** as Jira reports them, e.g. `IMP`.
- `[projects]` is optional; omitting it keeps the global tables in charge.

Built-in defaults, used when no table maps a value:

| Jira status | aven |
|---|---|
| to do / open / new / backlog | todo |
| in progress / in review | active |
| done / resolved / closed | done |
| cancelled / canceled | canceled |
| anything else (treated as open) | todo |

| Jira priority | aven |
|---|---|
| highest / urgent | urgent |
| high | high |
| medium | medium |
| low / lowest | low |

Full example — global tables plus a per-project override:

```toml
[status_map]
"In Review" = "todo"          # global: every project

[priority_map]
"Highest" = "urgent"

[project_map]
"IMP" = "improvements"        # aven project name

[projects.IMP.status_map]
"In Review" = "active"        # wins over the global table for IMP only
```

Per new task, the tool writes metadata `jira-key`, `jira-url`, `jira-status`, and always adds the `jira` label plus the issue's Jira labels.
