#!/bin/sh
# Cron wrapper for jira-aven-sync. Sources the token env file (never committed),
# then runs the installed binary with the repo's config. Idempotent — safe to run often.
set -eu
. "$HOME/.config/jira-aven-sync/env"
export JIRA_API_TOKEN
exec "$HOME/.cargo/bin/jira-aven-sync" --config "$HOME/Code/personal/jira-aven-sync/config.toml"
