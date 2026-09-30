//! Lane assignment: one sweep down the ordered rows.
//!
//! Edges span from the centre of row i to the centre of row i+1 and are
//! stored on row i. Slots are never compacted, so a branch keeps its column
//! for its whole life; freed slots are reused. Lane 0 is reserved for HEAD's
//! first-parent line until HEAD has been placed.
//!
//! A lane can carry several branch tips, as when one branch was started from
//! the tip of another. The top-most tip shares the lane's colour. Each tip
//! below it gets a colour of its own: on its pills only
//! (`CommitLaneColours::KeepLane`), or on the lane from that tip down too
//! (`CommitLaneColours::ChangeAtEachTip`).

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use specta::Type;

use super::oid::Oid;
use crate::settings::CommitLaneColours;

pub const PALETTE_SIZE: u8 = 12;
/// Distinct hues in the palette: the dark theme repeats lanes 0 to 5 as 6 to 11, so two indices
/// with the same remainder look the same.
const HUE_COUNT: u8 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum EdgeKind {
    /// A lane not involved with this commit continuing downward (may curve
    /// when it converges into the next row's dot).
    Pass,
    /// From this commit's dot to a parent in another lane (a fork).
    Parent,
    /// From this commit's dot into a lane that already expects that parent.
    MergeInto,
    /// The lane's parent is not in the snapshot; drawn as a fading stub.
    Dangling,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Edge {
    pub from_lane: u16,
    pub to_lane: u16,
    pub colour: u8,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowLayout {
    pub lane: u16,
    /// Colour of the commit's dot and of its lane at this row.
    pub colour: u8,
    /// Colour of the ref pills on this row. It differs from `colour` only on a branch tip below
    /// the lane's top-most one, with `CommitLaneColours::KeepLane`.
    pub ref_colour: u8,
    /// The branch this commit belongs to in the graph, as the caller's ref index: the nearest
    /// branch tip above it on its lane. None on a lane that no branch tip has reached, such as
    /// the second parent of a merge whose branch was deleted.
    pub branch: Option<u32>,
    /// Highest active lane index while this row is drawn (for column width).
    pub max_lane: u16,
    pub edge_start: u32,
    pub edge_len: u16,
}

#[derive(Debug, Clone)]
struct Slot {
    expects: Oid,
    colour: u8,
    /// Set for a slot forked this row: its edge starts at the forking dot.
    born_from: Option<u16>,
    /// The nearest branch tip placed on this lane so far, as the caller's ref index.
    branch: Option<u32>,
}

struct Pending {
    row: usize,
    dot_lane: u16,
    /// (from dot lane, target slot) merge edges resolved on the next push.
    extra: Vec<(u16, u16)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenLane {
    pub lane: u16,
    pub colour: u8,
    pub expects: Oid,
}

pub struct LaneSweep {
    slots: Vec<Option<Slot>>,
    expecting: FxHashMap<Oid, SmallVec<[u16; 2]>>,
    head: Option<Oid>,
    head_placed: bool,
    pending: Option<Pending>,
    next_colour: u8,
    lane_colours: CommitLaneColours,
    next_tip_colour: u8,
    pub rows: Vec<RowLayout>,
    pub edges: Vec<Edge>,
    pub max_lanes: u16,
}

impl LaneSweep {
    /// `head` reserves lane 0 for HEAD's line; pass None when HEAD is not in
    /// the set of rows that will be pushed.
    pub fn new(head: Option<Oid>, capacity: usize) -> LaneSweep {
        LaneSweep {
            slots: Vec::with_capacity(8),
            expecting: FxHashMap::default(),
            head,
            head_placed: head.is_none(),
            pending: None,
            next_colour: 0,
            lane_colours: CommitLaneColours::default(),
            next_tip_colour: 0,
            rows: Vec::with_capacity(capacity),
            edges: Vec::with_capacity(capacity * 2),
            max_lanes: 0,
        }
    }

    pub fn with_lane_colours(mut self, lane_colours: CommitLaneColours) -> LaneSweep {
        self.lane_colours = lane_colours;
        self
    }

    fn active_count(&self) -> u16 {
        self.slots.iter().filter(|s| s.is_some()).count() as u16
    }

    fn highest_active(&self) -> u16 {
        self.slots
            .iter()
            .rposition(|s| s.is_some())
            .map(|i| i as u16)
            .unwrap_or(0)
    }

    fn pick_colour(&mut self) -> u8 {
        let used: Vec<u8> = self.slots.iter().flatten().map(|s| s.colour).collect();
        let mut c = self.next_colour;
        for _ in 0..PALETTE_SIZE {
            if !used.contains(&c) {
                break;
            }
            c = (c + 1) % PALETTE_SIZE;
        }
        self.next_colour = (c + 1) % PALETTE_SIZE;
        c
    }

    /// Colour for a branch tip below the top of a lane coloured `lane`. Its hue always differs
    /// from the lane's, and from every open lane's while a hue is left over.
    fn pick_tip_colour(&mut self, lane: u8) -> u8 {
        let open: Vec<u8> = self.slots.iter().flatten().map(|s| s.colour % HUE_COUNT).collect();
        let candidates = (0..PALETTE_SIZE).map(|i| (self.next_tip_colour + i) % PALETTE_SIZE);
        let other_hue = |c: &u8| c % HUE_COUNT != lane % HUE_COUNT;
        let c = candidates
            .clone()
            .filter(other_hue)
            .find(|c| !open.contains(&(c % HUE_COUNT)))
            .or_else(|| candidates.clone().find(other_hue))
            .unwrap_or(lane);
        self.next_tip_colour = (c + 1) % PALETTE_SIZE;
        c
    }

    /// `wants_zero` is true for HEAD itself and for the Working Tree row that
    /// sits directly above it; both claim the reserved lane 0.
    fn alloc_slot(&mut self, expects: Oid, colour: u8, born_from: Option<u16>, wants_zero: bool) -> u16 {
        let reserve_zero = !self.head_placed;
        let head_wants_zero = reserve_zero && wants_zero;
        let start = if reserve_zero && !head_wants_zero { 1 } else { 0 };
        let slot = Slot { expects, colour, born_from, branch: None };
        if head_wants_zero {
            if self.slots.is_empty() {
                self.slots.push(None);
            }
            if self.slots[0].is_none() {
                self.slots[0] = Some(slot);
                return 0;
            }
        }
        for i in start..self.slots.len() {
            if self.slots[i].is_none() {
                self.slots[i] = Some(slot);
                return i as u16;
            }
        }
        if self.slots.is_empty() && start == 1 {
            self.slots.push(None);
        }
        self.slots.push(Some(slot));
        (self.slots.len() - 1) as u16
    }

    fn free(&mut self, lane: u16) {
        if let Some(Some(s)) = self.slots.get(lane as usize) {
            let oid = s.expects;
            if let Some(v) = self.expecting.get_mut(&oid) {
                v.retain(|l| *l != lane);
                if v.is_empty() {
                    self.expecting.remove(&oid);
                }
            }
        }
        self.slots[lane as usize] = None;
        while self.slots.last().is_some_and(|s| s.is_none()) {
            self.slots.pop();
        }
    }

    /// Emit the edges for the span from the previous row to this one.
    fn finalize_previous(&mut self, arriving: &[u16], lane_c: Option<u16>) {
        let Some(prev) = self.pending.take() else { return };
        let start = self.edges.len();
        for k in 0..self.slots.len() {
            let Some(slot) = self.slots[k].as_mut() else { continue };
            let k16 = k as u16;
            let from = slot.born_from.take().unwrap_or(k16);
            let to = if arriving.contains(&k16) { lane_c.unwrap_or(k16) } else { k16 };
            let kind = if from == prev.dot_lane && k16 != prev.dot_lane {
                EdgeKind::Parent
            } else {
                EdgeKind::Pass
            };
            self.edges.push(Edge { from_lane: from, to_lane: to, colour: slot.colour, kind });
        }
        for (from_dot, target) in prev.extra {
            let colour = self.slots.get(target as usize).and_then(|s| s.as_ref()).map(|s| s.colour).unwrap_or(0);
            let to = if arriving.contains(&target) { lane_c.unwrap_or(target) } else { target };
            self.edges.push(Edge { from_lane: from_dot, to_lane: to, colour, kind: EdgeKind::MergeInto });
        }
        let len = self.edges.len() - start;
        let row = &mut self.rows[prev.row];
        row.edge_start = start as u32;
        row.edge_len = len as u16;
    }

    pub fn push(&mut self, oid: Oid, parents: &[Oid]) -> RowLayout {
        self.push_row(oid, parents, None)
    }

    /// `tip` is the ref index of a branch that points at `oid`, when one does.
    pub fn push_row(&mut self, oid: Oid, parents: &[Oid], tip: Option<u32>) -> RowLayout {
        let mut arriving: SmallVec<[u16; 2]> = self.expecting.remove(&oid).unwrap_or_default();
        arriving.sort_unstable();
        let lane_c = arriving.first().copied();

        self.finalize_previous(&arriving, lane_c);

        let lane_c = match lane_c {
            Some(l) => l,
            None => {
                let colour = self.pick_colour();
                let wants_zero = self.head.is_some()
                    && (self.head == Some(oid) || (oid.is_worktree() && parents.first() == self.head.as_ref()));
                self.alloc_slot(oid, colour, None, wants_zero)
            }
        };
        if self.head == Some(oid) {
            self.head_placed = true;
        }
        // The edges above this row were drawn in the lane's old colour, so a change of lane
        // colour takes effect from this row's dot down.
        let lane_colour = self.slots[lane_c as usize].as_ref().map(|s| s.colour).unwrap_or(0);
        let below_a_tip = self.slots[lane_c as usize].as_ref().is_some_and(|s| s.branch.is_some());
        let tip_colour = (tip.is_some() && below_a_tip).then(|| self.pick_tip_colour(lane_colour));
        if let Some(s) = self.slots[lane_c as usize].as_mut() {
            s.branch = tip.or(s.branch);
            if let (Some(c), CommitLaneColours::ChangeAtEachTip) = (tip_colour, self.lane_colours) {
                s.colour = c;
            }
        }
        let colour_c = self.slots[lane_c as usize].as_ref().map(|s| s.colour).unwrap_or(0);
        let branch = self.slots[lane_c as usize].as_ref().and_then(|s| s.branch);

        // Lanes converging here end.
        for k in arriving.iter().skip(1) {
            self.free(*k);
        }

        let mut extra = Vec::new();
        match parents.first() {
            None => self.free(lane_c),
            Some(p0) => {
                if let Some(s) = self.slots[lane_c as usize].as_mut() {
                    s.expects = *p0;
                }
                self.expecting.entry(*p0).or_default().push(lane_c);
                for p in &parents[1..] {
                    let existing = self
                        .expecting
                        .get(p)
                        .and_then(|v| v.iter().copied().filter(|l| *l != lane_c).min());
                    match existing {
                        Some(j) => extra.push((lane_c, j)),
                        None => {
                            let colour = self.pick_colour();
                            let k = self.alloc_slot(*p, colour, Some(lane_c), false);
                            self.expecting.entry(*p).or_default().push(k);
                        }
                    }
                }
            }
        }

        let row = RowLayout {
            lane: lane_c,
            colour: colour_c,
            ref_colour: tip_colour.unwrap_or(colour_c),
            branch,
            max_lane: self.highest_active().max(lane_c),
            edge_start: 0,
            edge_len: 0,
        };
        self.max_lanes = self.max_lanes.max(self.active_count()).max(lane_c + 1);
        self.rows.push(row);
        self.pending = Some(Pending { row: self.rows.len() - 1, dot_lane: lane_c, extra });
        row
    }

    /// Close the sweep: every live lane after the last row is dangling.
    pub fn finish(&mut self) -> Vec<OpenLane> {
        if let Some(prev) = self.pending.take() {
            let start = self.edges.len();
            for k in 0..self.slots.len() {
                let Some(slot) = self.slots[k].as_mut() else { continue };
                let from = slot.born_from.take().unwrap_or(k as u16);
                self.edges.push(Edge { from_lane: from, to_lane: k as u16, colour: slot.colour, kind: EdgeKind::Dangling });
            }
            for (from_dot, target) in prev.extra {
                let colour = self.slots.get(target as usize).and_then(|s| s.as_ref()).map(|s| s.colour).unwrap_or(0);
                self.edges.push(Edge { from_lane: from_dot, to_lane: target, colour, kind: EdgeKind::Dangling });
            }
            let len = self.edges.len() - start;
            let row = &mut self.rows[prev.row];
            row.edge_start = start as u32;
            row.edge_len = len as u16;
        }
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().map(|s| OpenLane { lane: i as u16, colour: s.colour, expects: s.expects }))
            .collect()
    }

    pub fn edges_of(&self, row: &RowLayout) -> &[Edge] {
        let s = row.edge_start as usize;
        &self.edges[s..s + row.edge_len as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o(n: u8) -> Oid {
        let mut b = [0u8; 20];
        b[19] = n;
        Oid::from_bytes(&b).unwrap()
    }

    #[test]
    fn linear_history_stays_in_lane_zero() {
        let mut s = LaneSweep::new(Some(o(3)), 3);
        s.push(o(3), &[o(2)]);
        s.push(o(2), &[o(1)]);
        s.push(o(1), &[]);
        let open = s.finish();
        assert!(open.is_empty());
        assert!(s.rows.iter().all(|r| r.lane == 0));
        assert_eq!(s.max_lanes, 1);
        // Two spans, each one straight pass edge; the root has no edges below it.
        assert_eq!(s.edges_of(&s.rows[0]), &[Edge { from_lane: 0, to_lane: 0, colour: 0, kind: EdgeKind::Pass }]);
        assert_eq!(s.rows[2].edge_len, 0);
    }

    #[test]
    fn fork_and_merge_use_two_lanes_and_converge() {
        // m -> (c, d); c -> a; d -> a; a root.
        let mut s = LaneSweep::new(Some(o(9)), 4);
        s.push(o(9), &[o(7), o(8)]); // merge: first parent c=7 stays, d=8 forks to lane 1
        s.push(o(7), &[o(1)]);
        s.push(o(8), &[o(1)]);
        s.push(o(1), &[]);
        s.finish();
        assert_eq!(s.rows[0].lane, 0);
        assert_eq!(s.rows[1].lane, 0);
        assert_eq!(s.rows[2].lane, 1, "second parent lives in lane 1");
        assert_eq!(s.rows[3].lane, 0, "root converges back into lane 0");
        assert_eq!(s.max_lanes, 2);
        // Span m->c: lane 0 pass, lane 1 forked from the dot (Parent kind).
        let e0 = s.edges_of(&s.rows[0]);
        assert!(e0.contains(&Edge { from_lane: 0, to_lane: 0, colour: 0, kind: EdgeKind::Pass }));
        assert!(e0.iter().any(|e| e.from_lane == 0 && e.to_lane == 1 && e.kind == EdgeKind::Parent));
        // Span d->a: lane 1 converges into lane 0 (curve), lane 0 passes.
        let e2 = s.edges_of(&s.rows[2]);
        assert!(e2.iter().any(|e| e.from_lane == 1 && e.to_lane == 0));
        // Colours differ between the two lanes.
        assert_ne!(s.rows[1].colour, s.rows[2].colour);
    }

    #[test]
    fn merge_into_existing_lane_and_octopus() {
        // Tip x on lane 0 -> p; tip y (unrelated branch head) -> q; merge m(after) -> [p, q, r]
        let mut s = LaneSweep::new(Some(o(10)), 6);
        s.push(o(10), &[o(5), o(6), o(7)]); // octopus: 5 continues, 6 and 7 fork
        s.push(o(11), &[o(6)]);              // another tip whose parent is already expected -> converge later
        s.push(o(6), &[o(1)]);
        s.push(o(5), &[o(1)]);
        s.push(o(7), &[o(1)]);
        s.push(o(1), &[]);
        let open = s.finish();
        assert!(open.is_empty());
        assert_eq!(s.rows[0].lane, 0);
        assert!(s.max_lanes >= 3, "octopus needs at least three lanes, got {}", s.max_lanes);
        // Row for 6 has two lanes arriving (the fork from 10 and the tip 11).
        let r6 = s.rows[2];
        assert!(r6.lane >= 1);
    }

    #[test]
    fn dangling_parent_and_worktree_row() {
        let mut s = LaneSweep::new(Some(o(2)), 3);
        s.push(Oid::WORKTREE, &[o(2)]);
        s.push(o(2), &[o(1)]);
        s.push(o(1), &[o(99)]); // parent beyond the window
        let open = s.finish();
        assert_eq!(s.rows[0].lane, 0, "working tree row sits above HEAD in lane 0");
        assert_eq!(s.rows[1].lane, 0);
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].expects, o(99));
        let last = s.edges_of(&s.rows[2]);
        assert_eq!(last.len(), 1);
        assert_eq!(last[0].kind, EdgeKind::Dangling);
    }

    #[test]
    fn head_not_at_top_still_gets_lane_zero() {
        // A newer branch tip (o(5)) sits above HEAD (o(4)); HEAD's line must still be lane 0.
        let mut s = LaneSweep::new(Some(o(4)), 4);
        s.push(o(5), &[o(3)]);
        s.push(o(4), &[o(3)]);
        s.push(o(3), &[]);
        s.finish();
        assert_eq!(s.rows[0].lane, 1, "non-HEAD tip is pushed right while lane 0 is reserved");
        assert_eq!(s.rows[1].lane, 0);
        assert_eq!(s.rows[2].lane, 0, "shared parent converges into HEAD's lane");
    }

    /// Three branch tips on one first-parent line (4, 2 and 1), with a plain commit (3) between.
    fn stacked_tips(lane_colours: CommitLaneColours) -> LaneSweep {
        let mut s = LaneSweep::new(Some(o(4)), 4).with_lane_colours(lane_colours);
        s.push_row(o(4), &[o(3)], Some(4));
        s.push_row(o(3), &[o(2)], None);
        s.push_row(o(2), &[o(1)], Some(2));
        s.push_row(o(1), &[], Some(1));
        s.finish();
        s
    }

    fn hue(c: u8) -> u8 {
        c % HUE_COUNT
    }

    #[test]
    fn keep_lane_gives_lower_tips_their_own_pill_colour() {
        let s = stacked_tips(CommitLaneColours::KeepLane);
        let lane = s.rows[0].colour;
        assert!(s.rows.iter().all(|r| r.colour == lane), "the lane keeps one colour");
        assert!(s.edges.iter().all(|e| e.colour == lane));
        assert_eq!(s.rows[0].ref_colour, lane, "the top-most tip shares the lane's colour");
        assert_eq!(s.rows[1].ref_colour, lane, "a commit that is not a tip is unchanged");
        let (second, third) = (s.rows[2].ref_colour, s.rows[3].ref_colour);
        assert_ne!(hue(second), hue(lane));
        assert_ne!(hue(third), hue(lane));
        assert_ne!(hue(third), hue(second), "each lower tip differs from the one above it");
    }

    #[test]
    fn change_at_each_tip_recolours_the_lane_from_each_lower_tip_down() {
        let s = stacked_tips(CommitLaneColours::ChangeAtEachTip);
        let top = s.rows[0].colour;
        assert_eq!(s.rows[1].colour, top);
        assert_ne!(hue(s.rows[2].colour), hue(top));
        assert_ne!(hue(s.rows[3].colour), hue(s.rows[2].colour));
        assert!(s.rows.iter().all(|r| r.ref_colour == r.colour), "pills match the lane");
        // The span into a tip belongs to the branch above it; the span below takes the new colour.
        assert_eq!(s.edges_of(&s.rows[1])[0].colour, top);
        assert_eq!(s.edges_of(&s.rows[2])[0].colour, s.rows[2].colour);
    }

    #[test]
    fn a_tip_on_a_new_lane_keeps_that_lanes_colour() {
        // Two unrelated tips, each the first on its own lane.
        let mut s = LaneSweep::new(Some(o(4)), 3);
        s.push_row(o(5), &[o(3)], Some(5));
        s.push_row(o(4), &[o(3)], Some(4));
        s.push_row(o(3), &[], None);
        s.finish();
        assert_eq!(s.rows[0].ref_colour, s.rows[0].colour);
        assert_eq!(s.rows[1].ref_colour, s.rows[1].colour);
    }

    #[test]
    fn each_row_belongs_to_the_nearest_branch_tip_on_its_lane() {
        // master (10) -> 9 -> 3; feature (8) started from 3; lanes 0 and 1 meet at 3.
        let mut s = LaneSweep::new(Some(o(10)), 4);
        s.push_row(o(10), &[o(9)], Some(100));
        s.push_row(o(8), &[o(3)], Some(200));
        s.push_row(o(9), &[o(3)], None);
        s.push_row(o(3), &[], None);
        s.finish();
        let branches: Vec<Option<u32>> = s.rows.iter().map(|r| r.branch).collect();
        assert_eq!(branches, vec![Some(100), Some(200), Some(100), Some(100)], "the fork point stays master's");

        // A merge on master (20) whose second parent (18) is on a lane no branch tip reached.
        let mut s = LaneSweep::new(Some(o(20)), 4);
        s.push_row(o(20), &[o(19), o(18)], Some(100));
        s.push_row(o(19), &[o(1)], None);
        s.push_row(o(18), &[o(1)], None);
        s.push_row(o(1), &[], None);
        s.finish();
        let branches: Vec<Option<u32>> = s.rows.iter().map(|r| r.branch).collect();
        assert_eq!(branches, vec![Some(100), Some(100), None, Some(100)]);
    }
}
