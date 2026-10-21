//! An immutable, versioned view of one repository's graph. Built whole on
//! every refresh (about 25 ms for 100k commits) and swapped atomically.

use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use specta::Type;

use super::lanes::{Edge, LaneSweep, RowLayout};
use super::oid::Oid;
use super::order::order;
use super::store::CommitStore;
use crate::git::queries::refs::{RefKind, RefsSnapshot};
use crate::settings::GraphOptions;

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
    /// Palette index of this pill: the row's `ref_colour`, except that a remote branch tracked by
    /// a local branch shown in the graph takes that local branch's colour.
    pub colour: u8,
    /// The local branch's upstream points at the same commit, so one pill
    /// (`origin | dev`) stands for both.
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
    /// Colour of the pills that start a new colour on this row; see `RowLayout::ref_colour`. Each
    /// `RefLabel` carries its own, which differs for a tracked remote branch.
    pub ref_colour: u8,
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
    /// Pill colour (palette index 0 to 11) of every ref shown in the graph, keyed by full ref
    /// name; see `RefLabel::colour`. Hidden refs, and refs whose commit is not in the graph,
    /// are absent.
    pub ref_colours: std::collections::BTreeMap<String, u8>,
    /// The longest distinct author names (by character count, at most 24), so the frontend can
    /// size the Author column to fit every row without measuring the whole history.
    pub widest_authors: Vec<String>,
}

/// Full names of the refs (branches, remote branches, tags) switched off in the graph.
pub type HiddenRefs = FxHashSet<String>;

/// Colours the user chose for refs, by full name: a hue, palette index 0 to 11. Kept in the app's
/// UI state (`perRepo[key].refColours`), never in the repository.
pub type RefColours = FxHashMap<String, u8>;

pub struct GraphSnapshot {
    pub version: u64,
    /// Row index -> commit index in `store` (u32::MAX for the Working Tree row).
    pub order: Vec<u32>,
    /// With refs hidden, the display order of every commit in `store`, so the cache keeps the
    /// positions of the commits that are not shown. None when nothing is hidden (`order` is it).
    pub full_order: Option<Vec<u32>>,
    pub layout: Vec<RowLayout>,
    pub edges: Vec<Edge>,
    pub row_by_oid: FxHashMap<Oid, u32>,
    pub store: Arc<CommitStore>,
    pub refs: Arc<RefsSnapshot>,
    /// Indices into `refs.refs`, grouped by target commit.
    refs_by_target: FxHashMap<Oid, Vec<u32>>,
    /// Remote branch full name -> index in `refs.refs` of a shown local branch that tracks it.
    tracked_by: FxHashMap<String, u32>,
    /// The colours the user chose for refs; see `RefColours`.
    chosen: RefColours,
    pub head: Option<Oid>,
    pub worktree: Option<WorktreeSummary>,
    pub complete: bool,
    pub truncated: bool,
    pub max_lanes: u16,
    pub open_lanes: u32,
}

impl GraphSnapshot {
    /// `build_coloured` with no colours chosen.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        store: Arc<CommitStore>,
        refs: Arc<RefsSnapshot>,
        worktree: Option<WorktreeSummary>,
        options: GraphOptions,
        version: u64,
        complete: bool,
        truncated: bool,
        hidden: &HiddenRefs,
    ) -> GraphSnapshot {
        Self::build_coloured(store, refs, worktree, options, version, complete, truncated, hidden, &RefColours::default())
    }

    /// Lays out the graph. `hidden` refs are left out, with the commits only they reach, and the
    /// refs in `chosen` take the colours the user gave them: a branch colours its lane (see
    /// `LaneSweep::push_row`), and any ref its own pill.
    #[allow(clippy::too_many_arguments)]
    pub fn build_coloured(
        store: Arc<CommitStore>,
        refs: Arc<RefsSnapshot>,
        worktree: Option<WorktreeSummary>,
        options: GraphOptions,
        version: u64,
        complete: bool,
        truncated: bool,
        hidden: &HiddenRefs,
        chosen: &RefColours,
    ) -> GraphSnapshot {
        let full = order(&store, options.order);
        // With refs hidden, show only commits reachable from the refs still shown. HEAD counts
        // as a tip of its own only when detached; on a branch, that branch's switch decides.
        let visible = (!hidden.is_empty()).then(|| {
            let mut tips: Vec<Oid> = refs
                .refs
                .iter()
                .filter(|r| !hidden.contains(&r.full_name))
                .filter_map(|r| Oid::parse_hex(&r.target))
                .collect();
            if refs.head_ref.is_none() {
                tips.extend(refs.head.as_deref().and_then(Oid::parse_hex));
            }
            store.reachable_from(&tips)
        });
        let (commit_order, full_order) = match &visible {
            Some(keep) => (full.iter().copied().filter(|ci| keep[*ci as usize]).collect(), Some(full)),
            None => (full, None),
        };
        let shown = |oid: &Oid| store.index_of(oid).is_some_and(|i| visible.as_ref().is_none_or(|keep| keep[i as usize]));
        let head = refs.head.as_deref().and_then(Oid::parse_hex).filter(|h| shown(h));

        let mut refs_by_target: FxHashMap<Oid, Vec<u32>> = FxHashMap::default();
        for (i, r) in refs.refs.iter().enumerate() {
            if hidden.contains(&r.full_name) {
                continue;
            }
            if let Some(t) = Oid::parse_hex(&r.target) {
                refs_by_target.entry(t).or_default().push(i as u32);
            }
        }
        let mut tracked_by: FxHashMap<String, u32> = FxHashMap::default();
        for (i, r) in refs.refs.iter().enumerate() {
            if let (RefKind::LocalBranch, Some(up)) = (&r.kind, &r.upstream) {
                if !hidden.contains(&r.full_name) && !hidden.contains(up) && !r.upstream_gone {
                    tracked_by.entry(up.clone()).or_insert(i as u32);
                }
            }
        }
        // The branch tip at a commit, as an index into `refs.refs`: the checked-out branch, then
        // any local branch, then a remote branch. A remote branch that a shown local branch
        // tracks is not a tip of its own: it wears the local branch's colour, so it neither takes
        // a new colour nor changes the lane's, and the lane belongs to the local branch.
        let branch_tip = |oid: &Oid| -> Option<u32> {
            let rank = |i: u32| {
                let r = &refs.refs[i as usize];
                match r.kind {
                    RefKind::LocalBranch => Some(if r.is_head { 0 } else { 1 }),
                    RefKind::RemoteBranch { .. } if !tracked_by.contains_key(&r.full_name) => Some(2),
                    _ => None,
                }
            };
            refs_by_target.get(oid)?.iter().filter_map(|&i| rank(i).map(|k| (k, i))).min().map(|(_, i)| i)
        };

        let extra = usize::from(worktree.is_some());
        let mut sweep = LaneSweep::new(head, commit_order.len() + extra)
            .with_lane_colours(options.lane_colours)
            .with_first_colour(options.first_colour);
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
            let tip = branch_tip(&c.oid);
            let colour = tip.and_then(|i| chosen.get(&refs.refs[i as usize].full_name).copied());
            sweep.push_row(c.oid, &c.parents, tip, colour);
            row_by_oid.insert(c.oid, order_rows.len() as u32);
            order_rows.push(ci);
        }
        let open = sweep.finish();

        GraphSnapshot {
            version,
            order: order_rows,
            full_order,
            max_lanes: sweep.max_lanes,
            layout: std::mem::take(&mut sweep.rows),
            edges: std::mem::take(&mut sweep.edges),
            row_by_oid,
            store,
            refs,
            refs_by_target,
            tracked_by,
            chosen: chosen.iter().filter(|(_, c)| **c < crate::graph::lanes::HUE_COUNT).map(|(k, c)| (k.clone(), *c)).collect(),
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
            ref_colours: self.ref_colours(),
            widest_authors: self.widest_authors(),
        }
    }

    fn widest_authors(&self) -> Vec<String> {
        let mut names: Vec<&str> = self.store.authors.iter().map(|a| &*a.name).collect();
        names.sort_unstable_by(|a, b| b.chars().count().cmp(&a.chars().count()).then(a.cmp(b)));
        names.dedup();
        names.into_iter().take(24).map(String::from).collect()
    }

    /// Short name of the branch a commit belongs to in the graph: the nearest branch tip above it
    /// on its lane (`RowLayout::branch`). None when the commit is not shown, or no tip reaches it.
    pub fn branch_of(&self, oid: &str) -> Option<&str> {
        let row = Oid::parse_hex(oid).and_then(|o| self.row_by_oid.get(&o).copied())?;
        let i = self.layout.get(row as usize)?.branch?;
        Some(self.refs.refs[i as usize].short.as_str())
    }

    /// Row layout of the commit a ref points at, when that commit is in the graph.
    fn layout_at(&self, target: &str) -> Option<&RowLayout> {
        let row = Oid::parse_hex(target).and_then(|t| self.row_by_oid.get(&t).copied())?;
        self.layout.get(row as usize)
    }

    /// Pill colour of a shown ref; see `RefLabel::colour`. A colour the user chose comes first.
    fn colour_of(&self, r: &crate::git::queries::refs::RefEntry) -> Option<u8> {
        if let Some(c) = self.chosen.get(&r.full_name) {
            return self.layout_at(&r.target).map(|_| *c);
        }
        let local = self.tracked_by.get(&r.full_name).map(|i| &self.refs.refs[*i as usize]);
        if let Some(layout) = local.and_then(|l| self.layout_at(&l.target)) {
            return Some(layout.ref_colour);
        }
        self.layout_at(&r.target).map(|layout| layout.ref_colour)
    }

    fn ref_colours(&self) -> std::collections::BTreeMap<String, u8> {
        let mut out = std::collections::BTreeMap::new();
        for indices in self.refs_by_target.values() {
            for &i in indices {
                let r = &self.refs.refs[i as usize];
                if let Some(colour) = self.colour_of(r) {
                    out.insert(r.full_name.clone(), colour);
                }
            }
        }
        out
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
                colour: self.colour_of(r).unwrap_or(0),
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
                ref_colour: layout.ref_colour,
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
            ref_colour: layout.ref_colour,
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

    /// The text search behind the graph's Find box. A commit row matches when its subject, author
    /// name or author email contains `query` (ignoring case), or its oid starts with it. The
    /// search starts next to `from_row` in `direction` and wraps around the ends, so `from_row`
    /// itself is found last; with no `from_row` it starts at the top (or the bottom, backward).
    pub fn search(&self, query: &str, from_row: Option<u32>, direction: SearchDirection) -> GraphSearchResult {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return GraphSearchResult { row: None, matches: 0, rows: Vec::new() };
        }
        let oid_prefix = needle.bytes().all(|b| b.is_ascii_hexdigit()).then_some(needle.as_str());
        // Authors are interned, so each is tested once rather than once per commit.
        let author_hit: Vec<bool> = self
            .store
            .authors
            .iter()
            .map(|a| contains_ignore_case(&a.name, &needle) || contains_ignore_case(&a.email, &needle))
            .collect();
        let hits: Vec<u32> = (0..self.total())
            .filter(|&row| {
                let ci = self.order[row as usize];
                let Some(c) = self.store.get(ci) else { return false };
                author_hit[c.author as usize]
                    || contains_ignore_case(&c.subject, &needle)
                    || oid_prefix.is_some_and(|p| c.oid.to_hex().starts_with(p))
            })
            .collect();
        let row = match (direction, from_row) {
            (SearchDirection::Forward, None) => hits.first().copied(),
            (SearchDirection::Backward, None) => hits.last().copied(),
            (SearchDirection::Forward, Some(from)) => hits.iter().find(|&&r| r > from).or(hits.first()).copied(),
            (SearchDirection::Backward, Some(from)) => hits.iter().rev().find(|&&r| r < from).or(hits.last()).copied(),
        };
        GraphSearchResult { row, matches: hits.len() as u32, rows: hits }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum SearchDirection {
    Forward,
    Backward,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct GraphSearchResult {
    /// The matching row to select, or None when nothing matches.
    pub row: Option<u32>,
    /// How many rows match in the whole graph.
    pub matches: u32,
    /// Every matching row, top to bottom, so the search box can count them and step through
    /// them without asking again.
    pub rows: Vec<u32>,
}

/// Whether `hay` contains `needle`, which the caller has already lowercased.
fn contains_ignore_case(hay: &str, needle: &str) -> bool {
    if needle.is_ascii() {
        let n = needle.as_bytes();
        hay.len() >= n.len() && hay.as_bytes().windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
    } else {
        hay.to_lowercase().contains(needle)
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
        let snap = GraphSnapshot::build(Arc::new(store), Arc::new(refs), Some(wt), GraphOptions::default(), 7, true, false, &HiddenRefs::default());
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

    #[test]
    fn search_finds_subject_author_and_oid_in_either_direction_and_wraps() {
        let mut store = CommitStore::new();
        let mut c = rec("c", &["b"], 3);
        c.subject = "Fix the Parser".into();
        let mut b = rec("b", &["a"], 2);
        b.author_name = "Zoë Example".into();
        let mut a = rec("a", &[], 1);
        a.subject = "parser: first version".into();
        for r in [&c, &b, &a] {
            store.insert(r);
        }
        let wt = WorktreeSummary { staged: 0, unstaged: 1, untracked: 0, conflicted: 0 };
        let snap = GraphSnapshot::build(Arc::new(store), Arc::new(RefsSnapshot::default()), Some(wt), GraphOptions::default(), 1, true, false, &HiddenRefs::default());
        // Rows: 0 Working Tree, 1 c, 2 b, 3 a.
        use SearchDirection::{Backward, Forward};
        let hit = |q: &str, from: Option<u32>, d| snap.search(q, from, d);
        assert_eq!(hit("PARSER", None, Forward), GraphSearchResult { row: Some(1), matches: 2, rows: vec![1, 3] });
        assert_eq!(hit("parser", Some(1), Forward).row, Some(3));
        assert_eq!(hit("parser", Some(3), Forward).row, Some(1), "wraps past the bottom");
        assert_eq!(hit("parser", Some(1), Backward).row, Some(3), "wraps past the top");
        assert_eq!(hit("parser", None, Backward).row, Some(3));
        assert_eq!(hit("zoë", None, Forward).row, Some(2), "author names, beyond ASCII too");
        assert_eq!(hit("m@b", None, Forward).matches, 3, "author emails");
        assert_eq!(hit(&format!("{:0>40}", "a"), None, Forward).row, Some(3), "a whole oid");
        assert_eq!(hit(&"0".repeat(39), None, Forward).matches, 3, "an oid prefix");
        assert_eq!(hit("working", None, Forward).matches, 0, "the Working Tree row is not a commit");
        assert_eq!(hit("  ", None, Forward), GraphSearchResult { row: None, matches: 0, rows: Vec::new() });
    }

    #[test]
    fn hidden_refs_drop_their_commits_and_labels() {
        // a - b - c (main, checked out); b - d (feature); d is also tagged v2.
        let mut store = CommitStore::new();
        store.insert(&rec("a", &[], 1));
        store.insert(&rec("b", &["a"], 2));
        store.insert(&rec("c", &["b"], 3));
        store.insert(&rec("d", &["b"], 4));
        let refs = RefsSnapshot {
            refs: vec![
                r("refs/heads/main", "main", "c", RefKind::LocalBranch, None, true),
                r("refs/heads/feature", "feature", "d", RefKind::LocalBranch, None, false),
                r("refs/tags/v2", "v2", "d", RefKind::Tag { annotated: false }, None, false),
            ],
            head: Some(format!("{:0>40}", "c")),
            head_ref: Some("refs/heads/main".into()),
            detached: false,
            remotes: vec![],
        };
        let store = Arc::new(store);
        let refs = Arc::new(refs);
        let oid = |x: &str| format!("{x:0>40}");

        let all = GraphSnapshot::build(store.clone(), refs.clone(), None, GraphOptions::default(), 1, true, false, &HiddenRefs::default());
        assert_eq!(all.total(), 4);
        assert!(all.full_order.is_none());
        assert_eq!(all.meta().ref_colours.len(), 3);

        // Hiding the branch alone keeps d, because the tag still reaches it.
        let hidden: HiddenRefs = ["refs/heads/feature".to_string()].into_iter().collect();
        let snap = GraphSnapshot::build(store.clone(), refs.clone(), None, GraphOptions::default(), 2, true, false, &hidden);
        assert_eq!(snap.total(), 4);
        assert!(!snap.meta().ref_colours.contains_key("refs/heads/feature"));
        let d_row = snap.find(&oid("d")).unwrap();
        assert!(snap.rows(d_row, 1)[0].refs.iter().all(|l| l.short == "v2"), "feature's label is gone");

        // Hiding the tag too drops d; the cache order still has all four commits.
        let hidden: HiddenRefs = ["refs/heads/feature".to_string(), "refs/tags/v2".to_string()].into_iter().collect();
        let snap = GraphSnapshot::build(store.clone(), refs.clone(), None, GraphOptions::default(), 3, true, false, &hidden);
        assert_eq!(snap.total(), 3);
        assert_eq!(snap.find(&oid("d")), None);
        assert_eq!(snap.full_order.as_ref().map(Vec::len), Some(4));
        assert_eq!(snap.meta().ref_colours.keys().collect::<Vec<_>>(), vec!["refs/heads/main"]);

        // Hiding the checked-out branch hides its commits too; HEAD is not a tip of its own.
        let hidden: HiddenRefs = ["refs/heads/main".to_string()].into_iter().collect();
        let wt = WorktreeSummary { staged: 0, unstaged: 1, untracked: 0, conflicted: 0 };
        let snap = GraphSnapshot::build(store, refs, Some(wt), GraphOptions::default(), 4, true, false, &hidden);
        assert_eq!(snap.find(&oid("c")), None);
        assert_eq!(snap.find(&oid("d")), Some(1), "the working tree row, then d");
        assert!(snap.meta().head.is_none());
    }

    #[test]
    fn branches_stacked_on_one_lane_get_distinct_pill_colours() {
        // feature was started from dev's tip, and dev from base's: one straight line a-b-c-d.
        let mut store = CommitStore::new();
        store.insert(&rec("a", &[], 1));
        store.insert(&rec("b", &["a"], 2));
        store.insert(&rec("c", &["b"], 3));
        store.insert(&rec("d", &["c"], 4));
        let refs = RefsSnapshot {
            refs: vec![
                r("refs/heads/feature", "feature", "d", RefKind::LocalBranch, None, true),
                r("refs/tags/v1", "v1", "c", RefKind::Tag { annotated: false }, None, false),
                r("refs/heads/dev", "dev", "b", RefKind::LocalBranch, None, false),
                r("refs/heads/base", "base", "a", RefKind::LocalBranch, None, false),
            ],
            head: Some(format!("{:0>40}", "d")),
            head_ref: Some("refs/heads/feature".into()),
            detached: false,
            remotes: vec![],
        };
        let (store, refs) = (Arc::new(store), Arc::new(refs));

        let keep = GraphSnapshot::build(store.clone(), refs.clone(), None, GraphOptions::default(), 1, true, false, &HiddenRefs::default());
        let colours = keep.meta().ref_colours;
        let rows = keep.rows(0, 4);
        assert!(rows.iter().all(|r| r.colour == rows[0].colour), "the lane keeps one colour by default");
        assert_eq!(colours["refs/heads/feature"], rows[0].colour);
        assert_eq!(colours["refs/tags/v1"], rows[0].colour, "a tag alone does not start a new colour");
        let hues: Vec<u8> = ["feature", "dev", "base"].iter().map(|b| colours[&format!("refs/heads/{b}")]).collect();
        assert!(hues[0] != hues[1] && hues[1] != hues[2] && hues[0] != hues[2], "hues {hues:?}");

        let options = GraphOptions { lane_colours: crate::settings::CommitLaneColours::ChangeAtEachTip, ..GraphOptions::default() };
        let change = GraphSnapshot::build(store, refs, None, options, 2, true, false, &HiddenRefs::default());
        let rows = change.rows(0, 4);
        assert_eq!(rows[1].colour, rows[0].colour, "the tag row stays on feature's stretch");
        assert_ne!(rows[2].colour, rows[0].colour, "the lane changes colour at dev");
        assert!(rows.iter().all(|r| r.ref_colour == r.colour));
    }

    #[test]
    fn chosen_colours_go_on_branches_lanes_and_any_refs_pill() {
        let mut store = CommitStore::new();
        store.insert(&rec("a", &[], 1));
        store.insert(&rec("b", &["a"], 2));
        let refs = RefsSnapshot {
            refs: vec![
                r("refs/heads/main", "main", "b", RefKind::LocalBranch, None, true),
                r("refs/tags/v1", "v1", "a", RefKind::Tag { annotated: false }, None, false),
            ],
            head: Some(format!("{:0>40}", "b")),
            head_ref: Some("refs/heads/main".into()),
            detached: false,
            remotes: vec![],
        };
        let mut chosen = RefColours::default();
        chosen.insert("refs/heads/main".into(), 3);
        chosen.insert("refs/tags/v1".into(), 5);
        let snap = GraphSnapshot::build_coloured(Arc::new(store), Arc::new(refs), None, GraphOptions::default(), 1, true, false, &HiddenRefs::default(), &chosen);
        let colours = snap.meta().ref_colours;
        assert_eq!((colours["refs/heads/main"], colours["refs/tags/v1"]), (3, 5));
        let rows: Vec<GraphRow> = (0..snap.total()).filter_map(|i| snap.row(i)).collect();
        assert!(rows.iter().all(|row| row.colour == 3), "main's lane runs in its colour");
        assert_eq!(rows[1].refs[0].colour, 5, "the tag's pill");
    }

    /// A line a-b-c-d with `feature` and its upstream at `feature_at` and `remote_at`, and `base` at a.
    fn tracked_line(feature_at: &str, remote_at: &str, lane_colours: crate::settings::CommitLaneColours) -> GraphSnapshot {
        let mut store = CommitStore::new();
        store.insert(&rec("a", &[], 1));
        store.insert(&rec("b", &["a"], 2));
        store.insert(&rec("c", &["b"], 3));
        store.insert(&rec("d", &["c"], 4));
        let refs = RefsSnapshot {
            refs: vec![
                r("refs/heads/feature", "feature", feature_at, RefKind::LocalBranch, Some("refs/remotes/origin/feature"), true),
                r("refs/remotes/origin/feature", "origin/feature", remote_at, RefKind::RemoteBranch { remote: "origin".into() }, None, false),
                r("refs/heads/base", "base", "a", RefKind::LocalBranch, None, false),
            ],
            head: Some(format!("{feature_at:0>40}")),
            head_ref: Some("refs/heads/feature".into()),
            detached: false,
            remotes: vec![],
        };
        let options = GraphOptions { lane_colours, ..GraphOptions::default() };
        GraphSnapshot::build(Arc::new(store), Arc::new(refs), None, options, 1, true, false, &HiddenRefs::default())
    }

    #[test]
    fn a_tracked_remote_shares_its_local_branchs_colour() {
        use crate::settings::CommitLaneColours::{ChangeAtEachTip, KeepLane};
        // Local ahead of its remote: the remote below does not count as a tip of its own.
        let snap = tracked_line("d", "c", KeepLane);
        let colours = snap.meta().ref_colours;
        let rows = snap.rows(0, 4);
        assert_eq!(colours["refs/heads/feature"], rows[0].colour);
        assert_eq!(colours["refs/remotes/origin/feature"], colours["refs/heads/feature"]);
        assert_eq!(rows[1].refs[0].colour, colours["refs/heads/feature"], "the pill carries the colour too");
        assert_ne!(colours["refs/heads/base"], rows[0].colour, "base is still a lower tip");

        // Remote ahead of its local: the local is now the top-most tip and keeps the lane's colour.
        let snap = tracked_line("c", "d", KeepLane);
        let colours = snap.meta().ref_colours;
        assert_eq!(colours["refs/heads/feature"], snap.rows(0, 1)[0].colour);
        assert_eq!(colours["refs/remotes/origin/feature"], colours["refs/heads/feature"]);

        // Changing the lane at each tip: the remote's commit does not change the lane's colour.
        let snap = tracked_line("d", "c", ChangeAtEachTip);
        let rows = snap.rows(0, 4);
        assert_eq!(rows[1].colour, rows[0].colour);
        assert_eq!(rows[1].refs[0].colour, rows[0].colour);
        assert_ne!(rows[3].colour, rows[0].colour, "the lane still changes at base");
    }
}
