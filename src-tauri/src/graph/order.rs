//! Display order: the same result as `git log --date-order`, computed here so
//! cached and live loads agree and so Git never has to walk the whole history
//! before printing its first line.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use super::store::CommitStore;
use crate::settings::GraphOrder;

/// Row order as indices into `store.commits`. No parent appears before any
/// of its children, which the lane sweep relies on.
pub fn order(store: &CommitStore, mode: GraphOrder) -> Vec<u32> {
    let n = store.len();
    let mut children_left = vec![0u32; n];
    for c in &store.commits {
        for p in &c.parents {
            if let Some(pi) = store.index_of(p) {
                children_left[pi as usize] += 1;
            }
        }
    }
    // Max-heap on (commit_time, insertion order): newest first; equal times keep
    // Git's own streaming order (lower store index first), which is also how
    // `git log` breaks such ties.
    let mut heap: BinaryHeap<(i64, Reverse<u32>)> = BinaryHeap::new();
    for (i, c) in store.commits.iter().enumerate() {
        if children_left[i] == 0 {
            heap.push((c.commit_time, Reverse(i as u32)));
        }
    }
    let mut out = Vec::with_capacity(n);
    match mode {
        GraphOrder::Topo => {
            // Git's own `--topo-order`: the tips are taken newest first, but after that the next
            // row is always the commit made ready most recently (a stack, not a date heap), so a
            // branch's commits follow one another down to where it forked. A merge readies its
            // first parent before the others, so the merged branch is shown first, directly
            // under the merge, and the line it was merged into continues after it.
            let mut stack: Vec<u32> = heap.into_sorted_vec().into_iter().map(|(_, Reverse(i))| i).collect();
            while let Some(i) = stack.pop() {
                out.push(i);
                for p in &store.commits[i as usize].parents {
                    if let Some(pi) = store.index_of(p) {
                        children_left[pi as usize] -= 1;
                        if children_left[pi as usize] == 0 {
                            stack.push(pi);
                        }
                    }
                }
            }
        }
        GraphOrder::Date => {
            while let Some((_, Reverse(i))) = heap.pop() {
                out.push(i);
                for p in &store.commits[i as usize].parents {
                    if let Some(pi) = store.index_of(p) {
                        children_left[pi as usize] -= 1;
                        if children_left[pi as usize] == 0 {
                            let pc = &store.commits[pi as usize];
                            heap.push((pc.commit_time, Reverse(pi)));
                        }
                    }
                }
            }
        }
    }
    // Commits in a cycle (impossible in Git) or unreachable from any tip would
    // be missing; append them so nothing silently disappears.
    if out.len() < n {
        let mut seen = vec![false; n];
        for i in &out {
            seen[*i as usize] = true;
        }
        for (i, _) in seen.iter().enumerate().filter(|(_, seen)| !**seen) {
            out.push(i as u32);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::queries::log::CommitRecord;

    fn rec(oid: &str, parents: &[&str], t: i64) -> CommitRecord {
        CommitRecord {
            oid: format!("{oid:0>40}"),
            parents: parents.iter().map(|p| format!("{p:0>40}")).collect(),
            author_name: "a".into(),
            author_email: "a@b".into(),
            author_time: t,
            committer_name: "a".into(),
            committer_email: "a@b".into(),
            commit_time: t,
            subject: oid.into(),
        }
    }

    #[test]
    fn parents_never_precede_children_and_newest_comes_first() {
        let mut s = CommitStore::new();
        // Insert in a scrambled order; times deliberately out of topological order
        // for "b" (older child than its parent's timestamp would suggest).
        for r in [
            rec("a", &[], 100),
            rec("b", &["a"], 90),  // child with an older timestamp than parent
            rec("c", &["b"], 200),
            rec("d", &["a"], 150),
            rec("e", &["c", "d"], 300),
        ] {
            s.insert(&r);
        }
        let ord = order(&s, GraphOrder::Date);
        let names: Vec<&str> = ord.iter().map(|i| &*s.commits[*i as usize].subject).collect();
        assert_eq!(names[0], "e");
        let pos = |n: &str| names.iter().position(|x| *x == n).unwrap();
        assert!(pos("c") < pos("b"));
        assert!(pos("b") < pos("a"));
        assert!(pos("d") < pos("a"));
        assert_eq!(names.len(), 5);
    }

    #[test]
    fn topo_order_keeps_a_branch_together_when_dates_interleave() {
        // main: a(1) - b(3) - d(5) - 99(7), where the merge 99 brings in topic: c(2) - e(4) - f(6),
        // forked from a. The oids must be hexadecimal, hence 99 and 88.
        let mut s = CommitStore::new();
        for r in [
            rec("99", &["d", "f"], 7),
            rec("f", &["e"], 6),
            rec("d", &["b"], 5),
            rec("e", &["c"], 4),
            rec("b", &["a"], 3),
            rec("c", &["a"], 2),
            rec("a", &[], 1),
            // A second tip, older than the merge, on a branch of its own from b.
            rec("88", &["b"], 4),
        ] {
            s.insert(&r);
        }
        let names = |mode| order(&s, mode).iter().map(|i| s.commits[*i as usize].subject.to_string()).collect::<Vec<_>>().join(" ");
        assert_eq!(names(GraphOrder::Date), "99 f d e 88 b c a", "date order interleaves the two lines");
        assert_eq!(names(GraphOrder::Topo), "99 f e c d 88 b a", "the topic under the merge, then main");
    }
}
