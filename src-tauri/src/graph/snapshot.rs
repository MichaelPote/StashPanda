//! An immutable, versioned view of one repository's graph. Built whole on
//! every refresh (about 25 ms for 100k commits) and swapped atomically.

use std::sync::Arc;

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use specta::Type;

use super::lanes::{Edge, LaneSweep, RowLayout};
use super::oid::Oid;
use super::order::order;
use super::store::CommitStore;
use crate::git::queries::refs::{RefKind, RefsSnapshot};
use crate::settings::GraphOrder;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeSummary {
    pub staged: u32,
    pub unstaged: u32,
    pub untracked: u32,
    pub conflicted: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum RowKind {
    Commit,
    WorkingTree,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RefLabel {
    pub full_name: String,
    pub short: String,
    pub kind: RefKind,
    pub is_current: bool,
    /// The local branch's upstream points at the same commit, so one pill
    /// (`dev ▸ origin`) stands for both.
    pub upstream_at_same_commit: bool,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct GraphRow {
    pub index: u32,
    pub kind: RowKind,
    pub oid: String,
    pub parents: Vec<String>,
    pub lane: u16,
    pub colour: u8,
    pub max_lane: u16,
    pub edges: Vec<Edge>,
    pub author_name: String,
    pub author_email: String,
    pub author_time: i64,
    pub commit_time: i64,
    pub subject: String,
    pub refs: Vec<RefLabel>,
    pub is_head: bool,
    pub worktree: Option<WorktreeSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct GraphMeta {
    pub version: u64,
    pub total: u32,
    pub complete: bool,
    pub truncated: bool,
    pub max_lanes: u16,
    pub open_lanes: u32,
    pub head: Option<String>,
    pub head_ref: Option<String>,
    pub worktree: Option<WorktreeSummary>,
}

pub struct GraphSnapshot {
    pub version: u64,
    /// Row index -> commit index in `store` (u32::MAX for the Working Tree row).
    pub order: Vec<u32>,
    pub layout: Vec<RowLayout>,
    pub edges: Vec<Edge>,
    pub row_by_oid: FxHashMap<Oid, u32>,
    pub store: Arc<CommitStore>,
    pub refs: Arc<RefsSnapshot>,
    /// Indices into `refs.refs`, grouped by target commit.
    refs_by_target: FxHashMap<Oid, Vec<u32>>,
    pub head: Option<Oid>,
    pub worktree: Option<WorktreeSummary>,
    pub complete: bool,
    pub truncated: bool,
    pub max_lanes: u16,
    pub open_lanes: u32,
}

impl GraphSnapshot {
    pub fn build(
        store: Arc<CommitStore>,
        refs: Arc<RefsSnapshot>,
        worktree: Option<WorktreeSummary>,
        mode: GraphOrder,
        version: u64,
        complete: bool,
        truncated: bool,
    ) -> GraphSnapshot {
        let head = refs.head.as_deref().and_then(Oid::parse_hex).filter(|h| store.index_of(h).is_some());
        let commit_order = order(&store, mode);

        let extra = usize::from(worktree.is_some());
        let mut sweep = LaneSweep::new(head, commit_order.len() + extra);
        let mut order_rows: Vec<u32> = Vec::with_capacity(commit_order.len() + extra);
        let mut row_by_oid: FxHashMap<Oid, u32> = FxHashMap::default();
        row_by_oid.reserve(commit_order.len() + extra);

        if worktree.is_some() {
            let parents: Vec<Oid> = head.into_iter().collect();
            sweep.push(Oid::WORKTREE, &parents);
            row_by_oid.insert(Oid::WORKTREE, 0);
            order_rows.push(u32::MAX);
        }
        for ci in commit_order {
            let c = &store.commits[ci as usize];
            sweep.push(c.oid, &c.parents);
            row_by_oid.insert(c.oid, order_rows.len() as u32);
            order_rows.push(ci);
        }
        let open = sweep.finish();

        let mut refs_by_target: FxHashMap<Oid, Vec<u32>> = FxHashMap::default();
        for (i, r) in refs.refs.iter().enumerate() {
            if let Some(t) = Oid::parse_hex(&r.target) {
                refs_by_target.entry(t).or_default().push(i as u32);
            }
        }

        GraphSnapshot {
            version,
            order: order_rows,
            max_lanes: sweep.max_lanes,
            layout: std::mem::take(&mut sweep.rows),
            edges: std::mem::take(&mut sweep.edges),
            row_by_oid,
            store,
            refs,
            refs_by_target,
            head,
            worktree,
            complete,
            truncated,
            open_lanes: open.len() as u32,
        }
    }

    pub fn total(&self) -> u32 {
        self.order.len() as u32
    }

    pub fn meta(&self) -> GraphMeta {
        GraphMeta {
            version: self.version,
            total: self.total(),
            complete: self.complete,
            truncated: self.truncated,
            max_lanes: self.max_lanes,
            open_lanes: self.open_lanes,
            head: self.head.map(|h| h.to_hex()),
            head_ref: self.refs.head_ref.clone(),
            worktree: self.worktree,
        }
    }

    pub fn find(&self, oid: &str) -> Option<u32> {
        let o = if oid == "WORKTREE" { Some(Oid::WORKTREE) } else { Oid::parse_hex(oid) }?;
        self.row_by_oid.get(&o).copied()
    }

    fn labels_for(&self, oid: &Oid) -> Vec<RefLabel> {
        let Some(idx) = self.refs_by_target.get(oid) else { return Vec::new() };
        let here: Vec<&crate::git::queries::refs::RefEntry> = idx.iter().map(|i| &self.refs.refs[*i as usize]).collect();
        let mut out = Vec::with_capacity(here.len());
        // Remote branches that are the upstream of a local branch on this same
        // commit collapse into that local branch's pill.
        let mut absorbed: Vec<&str> = Vec::new();
        for r in &here {
            if let (RefKind::LocalBranch, Some(up)) = (&r.kind, &r.upstream) {
                if here.iter().any(|o| &o.full_name == up) {
                    absorbed.push(up.as_str());
                }
            }
        }
        for r in &here {
            if absorbed.contains(&r.full_name.as_str()) {
                continue;
            }
            let upstream_at_same_commit = matches!(r.kind, RefKind::LocalBranch)
                && r.upstream.as_deref().is_some_and(|u| absorbed.contains(&u));
            out.push(RefLabel {
                full_name: r.full_name.clone(),
                short: r.short.clone(),
                kind: r.kind.clone(),
                is_current: r.is_head,
                upstream_at_same_commit,
                ahead: r.ahead,
                behind: r.behind,
            });
        }
        // Current branch first, then local, remote, tags.
        out.sort_by_key(|l| {
            let k = match l.kind {
                RefKind::LocalBranch => 1,
                RefKind::RemoteBranch { .. } => 2,
                RefKind::Tag { .. } => 3,
            };
            (if l.is_current { 0 } else { k }, l.short.clone())
        });
        out
    }

    pub fn row(&self, index: u32) -> Option<GraphRow> {
        let ci = *self.order.get(index as usize)?;
        let layout = self.layout[index as usize];
        let edges = self.edges[layout.edge_start as usize..(layout.edge_start + layout.edge_len as u32) as usize].to_vec();
        if ci == u32::MAX {
            return Some(GraphRow {
                index,
                kind: RowKind::WorkingTree,
                oid: "WORKTREE".into(),
                parents: self.head.map(|h| vec![h.to_hex()]).unwrap_or_default(),
                lane: layout.lane,
                colour: layout.colour,
                max_lane: layout.max_lane,
                edges,
                author_name: String::new(),
                author_email: String::new(),
                author_time: 0,
                commit_time: 0,
                subject: "Working Tree".into(),
                refs: Vec::new(),
                is_head: false,
                worktree: self.worktree,
            });
        }
        let c = self.store.get(ci)?;
        let author = self.store.author(c.author);
        Some(GraphRow {
            index,
            kind: RowKind::Commit,
            oid: c.oid.to_hex(),
            parents: c.parents.iter().map(|p| p.to_hex()).collect(),
            lane: layout.lane,
            colour: layout.colour,
            max_lane: layout.max_lane,
            edges,
            author_name: author.name.to_string(),
            author_email: author.email.to_string(),
            author_time: c.author_time,
            commit_time: c.commit_time,
            subject: c.subject.to_string(),
            refs: self.labels_for(&c.oid),
            is_head: self.head == Some(c.oid),
            worktree: None,
        })
    }

    pub fn rows(&self, start: u32, count: u32) -> Vec<GraphRow> {
        let end = (start.saturating_add(count)).min(self.total());
        (start..end).filter_map(|i| self.row(i)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::queries::log::CommitRecord;
    use crate::git::queries::refs::RefEntry;

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
            subject: format!("subject {oid}"),
        }
    }

    fn r(full: &str, short: &str, target: &str, kind: RefKind, upstream: Option<&str>, head: bool) -> RefEntry {
        RefEntry {
            full_name: full.into(),
            short: short.into(),
            target: format!("{target:0>40}"),
            kind,
            upstream: upstream.map(String::from),
            ahead: None,
            behind: None,
            upstream_gone: false,
            is_head: head,
            committer_time: 0,
            subject: String::new(),
            worktree_path: None,
        }
    }

    #[test]
    fn builds_rows_with_worktree_labels_and_lookup() {
        let mut store = CommitStore::new();
        store.insert(&rec("a", &[], 1));
        store.insert(&rec("b", &["a"], 2));
        store.insert(&rec("c", &["b"], 3));
        let refs = RefsSnapshot {
            refs: vec![
                r("refs/heads/main", "main", "c", RefKind::LocalBranch, Some("refs/remotes/origin/main"), true),
                r("refs/remotes/origin/main", "origin/main", "c", RefKind::RemoteBranch { remote: "origin".into() }, None, false),
                r("refs/tags/v1", "v1", "b", RefKind::Tag { annotated: false }, None, false),
            ],
            head: Some(format!("{:0>40}", "c")),
            head_ref: Some("refs/heads/main".into()),
            detached: false,
            remotes: vec![],
        };
        let wt = WorktreeSummary { staged: 1, unstaged: 2, untracked: 0, conflicted: 0 };
        let snap = GraphSnapshot::build(Arc::new(store), Arc::new(refs), Some(wt), GraphOrder::Date, 7, true, false);
        assert_eq!(snap.total(), 4);
        let rows = snap.rows(0, 10);
        assert_eq!(rows[0].kind, RowKind::WorkingTree);
        assert_eq!(rows[0].worktree, Some(wt));
        assert_eq!(rows[1].subject, "subject c");
        assert!(rows[1].is_head);
        assert_eq!(rows[1].refs.len(), 1, "origin/main collapses into main's pill");
        assert!(rows[1].refs[0].upstream_at_same_commit && rows[1].refs[0].is_current);
        assert_eq!(rows[2].refs[0].short, "v1");
        assert_eq!(snap.find(&format!("{:0>40}", "a")), Some(3));
        assert_eq!(snap.find("WORKTREE"), Some(0));
        assert_eq!(snap.meta().version, 7);
        assert!(rows.iter().all(|r| r.lane == 0));
    }
}
