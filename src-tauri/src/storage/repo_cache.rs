//! Read and write one repository's commits and refs in the SQLite cache.
//!
//! The cache exists so a tab paints its graph before Git answers. Commits are
//! stored once per (repo, oid); refs are replaced whole in the same
//! transaction, so the two are always consistent with each other.

use std::sync::Arc;

use rusqlite::{params, Connection, OptionalExtension};

use crate::git::queries::refs::{RefEntry, RefKind, RefsSnapshot};
use crate::graph::oid::Oid;
use crate::graph::snapshot::GraphSnapshot;
use crate::graph::store::{Author, CommitData, CommitStore};
use crate::storage::SCHEMA_VERSION;

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Row id for `key`, creating the repo row on first sight and stamping
/// `last_opened_at` either way.
pub fn ensure_repo(conn: &Connection, key: &str) -> rusqlite::Result<i64> {
    let now = now_secs();
    conn.execute(
        "INSERT INTO repos (key, last_opened_at, schema_version) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET last_opened_at = excluded.last_opened_at",
        params![key, now, SCHEMA_VERSION],
    )?;
    conn.query_row("SELECT id FROM repos WHERE key = ?1", params![key], |r| r.get(0))
}

pub struct CachedGraph {
    pub store: CommitStore,
    pub refs: RefsSnapshot,
    pub truncated: bool,
}

/// Everything cached for `key`, or None when nothing has been stored yet or
/// the row format is older than this build understands.
pub fn load(conn: &Connection, key: &str) -> rusqlite::Result<Option<CachedGraph>> {
    let row: Option<(i64, i64, i64, i64)> = conn
        .query_row(
            "SELECT id, oid_len, truncated, schema_version FROM repos WHERE key = ?1",
            params![key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((repo_id, oid_len, truncated, schema_version)) = row else { return Ok(None) };
    if schema_version != SCHEMA_VERSION {
        return Ok(None);
    }
    let oid_len = oid_len as usize;

    let mut store = CommitStore::new();
    {
        let mut stmt = conn.prepare(
            "SELECT oid, parents, author_name, author_email, author_time, commit_time, subject
             FROM commits WHERE repo_id = ?1 ORDER BY seq DESC",
        )?;
        let rows = stmt.query_map(params![repo_id], |r| {
            Ok((
                r.get::<_, Vec<u8>>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?;
        for row in rows {
            let (oid, parents, an, ae, at, ct, subject) = row?;
            let Some(oid) = Oid::from_bytes(&oid) else { continue };
            let parents = parents
                .chunks(oid_len.max(1))
                .filter_map(Oid::from_bytes)
                .collect();
            store.insert_data(
                CommitData { oid, parents, author: 0, author_time: at, commit_time: ct, subject: subject.into() },
                Author { name: an.into(), email: ae.into() },
            );
        }
    }
    if store.is_empty() {
        return Ok(None);
    }

    let mut refs = RefsSnapshot::default();
    {
        let mut stmt = conn.prepare(
            "SELECT name, target, tag_oid, kind, upstream, symbolic_target FROM refs WHERE repo_id = ?1",
        )?;
        let rows = stmt.query_map(params![repo_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get::<_, Option<Vec<u8>>>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        })?;
        for row in rows {
            let (name, target, tag_oid, kind, upstream, symbolic) = row?;
            let target_hex = Oid::from_bytes(&target).map(|o| o.to_hex()).unwrap_or_default();
            if kind == 3 {
                refs.head = Some(target_hex);
                refs.detached = symbolic.is_none();
                refs.head_ref = symbolic;
                continue;
            }
            let (short, kind) = match kind {
                0 => (name.trim_start_matches("refs/heads/").to_string(), RefKind::LocalBranch),
                1 => {
                    let s = name.trim_start_matches("refs/remotes/").to_string();
                    let remote = s.split('/').next().unwrap_or("").to_string();
                    (s, RefKind::RemoteBranch { remote })
                }
                2 => (name.trim_start_matches("refs/tags/").to_string(), RefKind::Tag { annotated: tag_oid.is_some() }),
                _ => continue,
            };
            refs.refs.push(RefEntry {
                full_name: name,
                short,
                target: target_hex,
                kind,
                upstream,
                ahead: None,
                behind: None,
                upstream_gone: false,
                is_head: false,
                committer_time: 0,
                subject: String::new(),
                worktree_path: None,
            });
        }
    }
    if let Some(h) = &refs.head_ref {
        for r in &mut refs.refs {
            r.is_head = &r.full_name == h;
        }
    }
    Ok(Some(CachedGraph { store, refs, truncated: truncated != 0 }))
}

/// Write a snapshot: new commits inserted, vanished commits deleted, refs
/// replaced, `seq` refreshed where it changed, repo metadata updated. One
/// transaction, so a crash leaves the previous consistent state.
pub fn persist(conn: &mut Connection, key: &str, snap: &Arc<GraphSnapshot>) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    let repo_id = ensure_repo(&tx, key)?;
    let oid_len = snap.store.commits.first().map(|c| c.oid.len()).unwrap_or(20) as i64;

    // Existing oids and their seq, to compute deletions and seq changes cheaply.
    let mut existing: rustc_hash::FxHashMap<Oid, Option<i64>> = rustc_hash::FxHashMap::default();
    {
        let mut stmt = tx.prepare("SELECT oid, seq FROM commits WHERE repo_id = ?1")?;
        let rows = stmt.query_map(params![repo_id], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Option<i64>>(1)?)))?;
        for row in rows {
            let (oid, seq) = row?;
            if let Some(o) = Oid::from_bytes(&oid) {
                existing.insert(o, seq);
            }
        }
    }

    // seq counts from the bottom so appends at the top change nothing below.
    let total_commit_rows = snap.order.iter().filter(|i| **i != u32::MAX).count() as i64;
    let mut seq_of: rustc_hash::FxHashMap<Oid, i64> = rustc_hash::FxHashMap::default();
    let mut pos = 0i64;
    for ci in &snap.order {
        if *ci == u32::MAX {
            continue;
        }
        let c = &snap.store.commits[*ci as usize];
        seq_of.insert(c.oid, total_commit_rows - 1 - pos);
        pos += 1;
    }

    {
        let mut insert = tx.prepare(
            "INSERT OR IGNORE INTO commits (repo_id, oid, parents, author_name, author_email, author_time, commit_time, subject, seq)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?;
        let mut update_seq = tx.prepare("UPDATE commits SET seq = ?3 WHERE repo_id = ?1 AND oid = ?2")?;
        for c in &snap.store.commits {
            let seq = seq_of.get(&c.oid).copied();
            match existing.get(&c.oid) {
                None => {
                    let mut parents = Vec::with_capacity(c.parents.len() * c.oid.len());
                    for p in &c.parents {
                        parents.extend_from_slice(p.as_bytes());
                    }
                    let a = snap.store.author(c.author);
                    insert.execute(params![
                        repo_id,
                        c.oid.as_bytes(),
                        parents,
                        &*a.name,
                        &*a.email,
                        c.author_time,
                        c.commit_time,
                        &*c.subject,
                        seq
                    ])?;
                }
                Some(old_seq) if *old_seq != seq => {
                    update_seq.execute(params![repo_id, c.oid.as_bytes(), seq])?;
                }
                _ => {}
            }
        }
        let mut delete = tx.prepare("DELETE FROM commits WHERE repo_id = ?1 AND oid = ?2")?;
        for (oid, _) in existing.iter() {
            if snap.store.index_of(oid).is_none() {
                delete.execute(params![repo_id, oid.as_bytes()])?;
            }
        }
    }

    tx.execute("DELETE FROM refs WHERE repo_id = ?1", params![repo_id])?;
    {
        let mut ins = tx.prepare(
            "INSERT INTO refs (repo_id, name, target, tag_oid, kind, upstream, symbolic_target) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for r in &snap.refs.refs {
            let Some(target) = Oid::parse_hex(&r.target) else { continue };
            let (kind, tag_oid): (i64, Option<Vec<u8>>) = match &r.kind {
                RefKind::LocalBranch => (0, None),
                RefKind::RemoteBranch { .. } => (1, None),
                RefKind::Tag { annotated } => (2, if *annotated { Some(Vec::new()) } else { None }),
            };
            ins.execute(params![repo_id, &r.full_name, target.as_bytes(), tag_oid, kind, &r.upstream, Option::<String>::None])?;
        }
        if let Some(h) = snap.refs.head.as_deref().and_then(Oid::parse_hex) {
            ins.execute(params![repo_id, "HEAD", h.as_bytes(), Option::<Vec<u8>>::None, 3i64, Option::<String>::None, &snap.refs.head_ref])?;
        }
    }

    tx.execute(
        "UPDATE repos SET oid_len = ?2, commit_count = ?3, last_refresh_at = ?4, last_full_load_at = COALESCE(last_full_load_at, ?4), truncated = ?5, schema_version = ?6 WHERE id = ?1",
        params![repo_id, oid_len, snap.store.len() as i64, now_secs(), snap.truncated as i64, SCHEMA_VERSION],
    )?;
    tx.commit()
}

/// Drop repositories that are no longer in the library, then the least
/// recently opened beyond `max_repos`.
pub fn evict(conn: &Connection, library_keys: &[String], max_repos: u32) -> rusqlite::Result<u32> {
    let mut removed = 0u32;
    let mut stmt = conn.prepare("SELECT id, key FROM repos ORDER BY last_opened_at DESC")?;
    let rows: Vec<(i64, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut kept = 0u32;
    for (id, key) in rows {
        let in_library = library_keys.iter().any(|k| *k == key);
        if in_library && kept < max_repos {
            kept += 1;
            continue;
        }
        conn.execute("DELETE FROM repos WHERE id = ?1", params![id])?;
        removed += 1;
    }
    if removed > 0 {
        let _ = conn.execute_batch("PRAGMA incremental_vacuum;");
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::queries::log::CommitRecord;
    use crate::settings::GraphOrder;
    use crate::storage::Db;

    fn rec(oid: &str, parents: &[&str], t: i64) -> CommitRecord {
        CommitRecord {
            oid: format!("{oid:0>40}"),
            parents: parents.iter().map(|p| format!("{p:0>40}")).collect(),
            author_name: "Michael".into(),
            author_email: "m@b".into(),
            author_time: t,
            committer_name: "Michael".into(),
            committer_email: "m@b".into(),
            commit_time: t,
            subject: format!("s{oid}"),
        }
    }

    fn snapshot(records: &[CommitRecord], head: &str, version: u64) -> Arc<GraphSnapshot> {
        let mut store = CommitStore::new();
        for r in records {
            store.insert(r);
        }
        let refs = RefsSnapshot {
            refs: vec![RefEntry {
                full_name: "refs/heads/main".into(),
                short: "main".into(),
                target: format!("{head:0>40}"),
                kind: RefKind::LocalBranch,
                upstream: Some("refs/remotes/origin/main".into()),
                ahead: None,
                behind: None,
                upstream_gone: false,
                is_head: true,
                committer_time: 0,
                subject: String::new(),
                worktree_path: None,
            }],
            head: Some(format!("{head:0>40}")),
            head_ref: Some("refs/heads/main".into()),
            detached: false,
            remotes: vec![],
        };
        Arc::new(GraphSnapshot::build(Arc::new(store), Arc::new(refs), None, GraphOrder::Date, version, true, false))
    }

    #[test]
    fn persist_then_load_round_trips_and_prunes() {
        let db = Db::open_in_memory().unwrap();
        let snap1 = snapshot(&[rec("a", &[], 1), rec("b", &["a"], 2), rec("c", &["b"], 3)], "c", 1);
        db.run_blocking(|c| persist(c, "k1", &snap1)).unwrap();

        let loaded = db.run_blocking(|c| load(c, "k1")).unwrap().expect("cached");
        assert_eq!(loaded.store.len(), 3);
        assert_eq!(loaded.refs.head.as_deref(), Some(format!("{:0>40}", "c").as_str()));
        assert_eq!(loaded.refs.head_ref.as_deref(), Some("refs/heads/main"));
        assert!(loaded.refs.refs[0].is_head);
        let b = loaded.store.index_of(&Oid::parse_hex(&format!("{:0>40}", "b")).unwrap()).unwrap();
        assert_eq!(loaded.store.commits[b as usize].parents.len(), 1);
        // Rebuilding a snapshot from the cache gives the same order as the live one.
        let rebuilt = GraphSnapshot::build(Arc::new(loaded.store), Arc::new(loaded.refs), None, GraphOrder::Date, 2, true, false);
        let live: Vec<String> = snap1.rows(0, 10).into_iter().map(|r| r.oid).collect();
        let cached: Vec<String> = rebuilt.rows(0, 10).into_iter().map(|r| r.oid).collect();
        assert_eq!(live, cached);

        // History rewritten: c dropped, d added on top of b.
        let snap2 = snapshot(&[rec("a", &[], 1), rec("b", &["a"], 2), rec("d", &["b"], 4)], "d", 3);
        db.run_blocking(|c| persist(c, "k1", &snap2)).unwrap();
        let loaded2 = db.run_blocking(|c| load(c, "k1")).unwrap().unwrap();
        assert_eq!(loaded2.store.len(), 3);
        assert!(loaded2.store.index_of(&Oid::parse_hex(&format!("{:0>40}", "c")).unwrap()).is_none());
        assert!(db.run_blocking(|c| load(c, "missing")).unwrap().is_none());

        // Eviction keeps library repos up to the limit and drops the rest.
        db.run_blocking(|c| persist(c, "k2", &snap2)).unwrap();
        let removed = db.run_blocking(|c| evict(c, &["k2".to_string()], 5)).unwrap();
        assert_eq!(removed, 1);
        assert!(db.run_blocking(|c| load(c, "k1")).unwrap().is_none());
    }
}
