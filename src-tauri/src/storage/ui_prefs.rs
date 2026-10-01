//! Remembered UI state in the `ui_prefs` and `repo_ui_prefs` tables (schema version 2).
//!
//! `UiState` is flattened to one row per value: nested objects become dotted keys
//! (`panels.leftWidth`), and anything else (numbers, strings, arrays such as `hiddenRefs`) is
//! stored as JSON. `perRepo` goes to `repo_ui_prefs`, one set of rows per repository. A null
//! value is not stored, so it reads back as the field's default.

use rusqlite::{params, Connection};
use serde_json::{Map, Value};

use crate::ui_state::UiState;

/// Appends one `(key, JSON)` pair per leaf of `value` under `prefix`.
fn flatten(prefix: &str, value: &Value, out: &mut Vec<(String, String)>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                flatten(&key, v, out);
            }
        }
        Value::Null => {}
        leaf => out.push((prefix.to_string(), leaf.to_string())),
    }
}

/// Puts `value` at the dotted `key` inside `root`, creating objects on the way.
fn insert_path(root: &mut Map<String, Value>, key: &str, value: Value) {
    let mut parts = key.split('.').peekable();
    let mut node = root;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            node.insert(part.to_string(), value);
            return;
        }
        let child = node.entry(part.to_string()).or_insert_with(|| Value::Object(Map::new()));
        if !child.is_object() {
            *child = Value::Object(Map::new());
        }
        node = child.as_object_mut().expect("object");
    }
}

/// Replaces every stored row with `state`, in one transaction.
pub fn save(conn: &mut Connection, state: &UiState) -> rusqlite::Result<()> {
    let mut json = serde_json::to_value(state).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let per_repo = json.as_object_mut().and_then(|o| o.remove("perRepo")).unwrap_or(Value::Null);

    let mut app_rows = Vec::new();
    flatten("", &json, &mut app_rows);

    let tx = conn.transaction()?;
    tx.execute("DELETE FROM ui_prefs", [])?;
    {
        let mut insert = tx.prepare("INSERT INTO ui_prefs (key, value) VALUES (?1, ?2)")?;
        for (key, value) in &app_rows {
            insert.execute(params![key, value])?;
        }
    }
    tx.execute("DELETE FROM repo_ui_prefs", [])?;
    if let Value::Object(repos) = per_repo {
        let mut insert = tx.prepare("INSERT INTO repo_ui_prefs (repo_key, key, value) VALUES (?1, ?2, ?3)")?;
        for (repo_key, repo_state) in &repos {
            let mut rows = Vec::new();
            flatten("", repo_state, &mut rows);
            for (key, value) in &rows {
                insert.execute(params![repo_key, key, value])?;
            }
        }
    }
    tx.commit()
}

/// The stored UI state as JSON, ready for `serde_json::from_value::<UiState>`, or None when both
/// tables are empty (a new database, before anything was saved or imported).
pub fn load(conn: &Connection) -> rusqlite::Result<Option<Value>> {
    let mut root = Map::new();
    let mut any = false;

    let mut stmt = conn.prepare("SELECT key, value FROM ui_prefs")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (key, text) = row?;
        any = true;
        // A value that is not valid JSON is skipped; the field then takes its default.
        if let Ok(value) = serde_json::from_str(&text) {
            insert_path(&mut root, &key, value);
        }
    }

    let mut per_repo = Map::new();
    let mut stmt = conn.prepare("SELECT repo_key, key, value FROM repo_ui_prefs")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))? {
        let (repo_key, key, text) = row?;
        any = true;
        if let Ok(value) = serde_json::from_str(&text) {
            let repo = per_repo.entry(repo_key).or_insert_with(|| Value::Object(Map::new()));
            if let Some(repo) = repo.as_object_mut() {
                insert_path(repo, &key, value);
            }
        }
    }
    root.insert("perRepo".into(), Value::Object(per_repo));

    Ok(any.then_some(Value::Object(root)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Db;
    use crate::ui_state::{DiffLayout, FilesGroupBy, RepoUiState};

    fn roundtrip(state: &UiState) -> UiState {
        let db = Db::open_in_memory().unwrap();
        let s = state.clone();
        db.run_blocking(move |c| save(c, &s)).unwrap();
        let json = db.run_blocking(|c| load(c)).unwrap().expect("rows were saved");
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn empty_tables_load_as_none() {
        let db = Db::open_in_memory().unwrap();
        assert!(db.run_blocking(|c| load(c)).unwrap().is_none());
    }

    #[test]
    fn state_survives_a_round_trip() {
        let mut state = UiState::default();
        state.panels.left_width = 31.5;
        state.graph_columns.author = Some(180);
        state.session.open_tabs = vec!["a".into(), "b".into()];
        state.console.log = false;
        state.console.watcher = true;
        state.files.group_by = FilesGroupBy::Flat;
        state.diff.layout = DiffLayout::Side;
        state.diff.split = 0.62;
        state.dialog_sizes.insert("commit".into(), crate::ui_state::DialogSize { width: 900, height: 640 });
        state.per_repo.insert(
            "repo-1".into(),
            RepoUiState { last_selected_oid: Some("abc".into()), hidden_refs: vec!["refs/remotes/origin/old".into()], pull_mode: None },
        );

        let back = roundtrip(&state);
        assert_eq!(back.panels.left_width, 31.5);
        assert_eq!(back.graph_columns.author, Some(180));
        assert_eq!(back.graph_columns.date, None);
        assert_eq!(back.session.open_tabs, vec!["a", "b"]);
        assert!(!back.console.log && back.console.watcher && !back.console.internal);
        assert_eq!(back.files.group_by, FilesGroupBy::Flat);
        assert_eq!(back.diff.layout, DiffLayout::Side);
        assert_eq!(back.diff.split, 0.62);
        assert_eq!((back.dialog_sizes["commit"].width, back.dialog_sizes["commit"].height), (900, 640));
        let repo = &back.per_repo["repo-1"];
        assert_eq!(repo.hidden_refs, vec!["refs/remotes/origin/old"]);
        assert_eq!(repo.last_selected_oid.as_deref(), Some("abc"));
    }

    #[test]
    fn one_row_per_value() {
        let db = Db::open_in_memory().unwrap();
        let mut state = UiState::default();
        state.per_repo.insert("repo-1".into(), RepoUiState { last_selected_oid: None, hidden_refs: vec!["refs/heads/x".into()], pull_mode: None });
        db.run_blocking(move |c| save(c, &state)).unwrap();
        let rows: Vec<(String, String)> = db
            .run_blocking(|c| {
                let mut stmt = c.prepare("SELECT key, value FROM ui_prefs WHERE key LIKE 'console.%' ORDER BY key")?;
                let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
                rows.collect()
            })
            .unwrap();
        assert_eq!(
            rows,
            vec![
                ("console.internal".into(), "false".into()),
                ("console.log".into(), "true".into()),
                ("console.userAction".into(), "false".into()),
                ("console.watcher".into(), "false".into()),
            ]
        );
        let repo_rows: Vec<(String, String, String)> = db
            .run_blocking(|c| {
                let mut stmt = c.prepare("SELECT repo_key, key, value FROM repo_ui_prefs")?;
                let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
                rows.collect()
            })
            .unwrap();
        // lastSelectedOid is None, so it has no row.
        assert_eq!(repo_rows, vec![("repo-1".into(), "hiddenRefs".into(), r#"["refs/heads/x"]"#.into())]);
    }

    #[test]
    fn a_removed_repository_loses_its_rows() {
        let db = Db::open_in_memory().unwrap();
        let mut state = UiState::default();
        state.per_repo.insert("gone".into(), RepoUiState { last_selected_oid: None, hidden_refs: vec!["refs/heads/x".into()], pull_mode: None });
        let first = state.clone();
        db.run_blocking(move |c| save(c, &first)).unwrap();
        state.per_repo.remove("gone");
        db.run_blocking(move |c| save(c, &state)).unwrap();
        let n: i64 = db.run_blocking(|c| c.query_row("SELECT COUNT(*) FROM repo_ui_prefs", [], |r| r.get(0))).unwrap();
        assert_eq!(n, 0);
    }
}
