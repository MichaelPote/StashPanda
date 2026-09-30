-- StashPanda schema version 2: remembered UI state (2026-09-30).
-- Applied by storage::Db::migrate when PRAGMA user_version is below 2.
--
-- One row per value. `key` is the value's path in `ui_state::UiState`, such as
-- `panels.leftWidth` or `console.internal`, and `value` is its JSON. The per-repository
-- table is keyed by RepoEntry.key from settings.json and deliberately does not reference
-- `repos`: that table is a cache whose rows are evicted, and UI state must outlive that.

CREATE TABLE IF NOT EXISTS ui_prefs (
  key   TEXT NOT NULL PRIMARY KEY,
  value TEXT NOT NULL
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS repo_ui_prefs (
  repo_key TEXT NOT NULL,
  key      TEXT NOT NULL,
  value    TEXT NOT NULL,
  PRIMARY KEY (repo_key, key)
) WITHOUT ROWID;
