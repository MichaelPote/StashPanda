-- StashPanda commit and ref cache. Version 1.
-- Applied by storage::Db::migrate when PRAGMA user_version is 0.

CREATE TABLE IF NOT EXISTS repos (
  id                INTEGER PRIMARY KEY,
  key               TEXT    NOT NULL UNIQUE,   -- RepoEntry.key from settings.json
  oid_len           INTEGER NOT NULL DEFAULT 20,
  commit_count      INTEGER NOT NULL DEFAULT 0,
  last_opened_at    INTEGER NOT NULL,          -- unix seconds; drives LRU eviction
  last_full_load_at INTEGER,
  last_refresh_at   INTEGER,
  truncated         INTEGER NOT NULL DEFAULT 0,
  schema_version    INTEGER NOT NULL           -- cache row format; mismatch => full reload
);

CREATE TABLE IF NOT EXISTS commits (
  repo_id      INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
  oid          BLOB    NOT NULL,               -- raw 20 or 32 bytes
  parents      BLOB    NOT NULL,               -- raw oids concatenated (n * oid_len bytes)
  author_name  TEXT    NOT NULL,
  author_email TEXT    NOT NULL,
  author_time  INTEGER NOT NULL,
  commit_time  INTEGER NOT NULL,
  subject      TEXT    NOT NULL,
  seq          INTEGER,                        -- display position counted from the root; NULL = not laid out
  PRIMARY KEY (repo_id, oid)
) WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS commits_repo_seq ON commits(repo_id, seq DESC);

CREATE TABLE IF NOT EXISTS refs (
  repo_id         INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
  name            TEXT    NOT NULL,            -- refs/heads/dev, refs/remotes/origin/dev, refs/tags/v1, HEAD
  target          BLOB    NOT NULL,            -- peeled commit oid
  tag_oid         BLOB,                        -- annotated tag object, if any
  kind            INTEGER NOT NULL,            -- 0 local, 1 remote, 2 tag, 3 HEAD, 4 stash
  upstream        TEXT,
  symbolic_target TEXT,                        -- HEAD -> refs/heads/dev (NULL when detached)
  PRIMARY KEY (repo_id, name)
) WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS refs_repo_target ON refs(repo_id, target);
