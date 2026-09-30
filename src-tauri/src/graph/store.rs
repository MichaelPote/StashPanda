//! In-memory commit store shared by the graph and the cache.

use rustc_hash::FxHashMap;
use smallvec::SmallVec;

use super::oid::Oid;
use crate::git::queries::log::CommitRecord;

#[derive(Debug, Clone)]
pub struct CommitData {
    pub oid: Oid,
    pub parents: SmallVec<[Oid; 2]>,
    pub author: u32,
    pub author_time: i64,
    pub commit_time: i64,
    pub subject: Box<str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Author {
    pub name: Box<str>,
    pub email: Box<str>,
}

#[derive(Default)]
pub struct CommitStore {
    pub commits: Vec<CommitData>,
    pub by_oid: FxHashMap<Oid, u32>,
    pub authors: Vec<Author>,
    author_index: FxHashMap<Author, u32>,
}

impl CommitStore {
    pub fn new() -> CommitStore {
        CommitStore::default()
    }

    pub fn len(&self) -> usize {
        self.commits.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commits.is_empty()
    }

    pub fn get(&self, idx: u32) -> Option<&CommitData> {
        self.commits.get(idx as usize)
    }

    pub fn index_of(&self, oid: &Oid) -> Option<u32> {
        self.by_oid.get(oid).copied()
    }

    pub fn author(&self, idx: u32) -> &Author {
        &self.authors[idx as usize]
    }

    fn intern_author(&mut self, name: &str, email: &str) -> u32 {
        let key = Author { name: name.into(), email: email.into() };
        if let Some(i) = self.author_index.get(&key) {
            return *i;
        }
        let i = self.authors.len() as u32;
        self.authors.push(key.clone());
        self.author_index.insert(key, i);
        i
    }

    /// Insert a parsed record. Returns false if the oid was already present.
    pub fn insert(&mut self, rec: &CommitRecord) -> bool {
        let Some(oid) = Oid::parse_hex(&rec.oid) else { return false };
        if self.by_oid.contains_key(&oid) {
            return false;
        }
        let parents: SmallVec<[Oid; 2]> = rec.parents.iter().filter_map(|p| Oid::parse_hex(p)).collect();
        let author = self.intern_author(&rec.author_name, &rec.author_email);
        let idx = self.commits.len() as u32;
        self.commits.push(CommitData {
            oid,
            parents,
            author,
            author_time: rec.author_time,
            commit_time: rec.commit_time,
            subject: rec.subject.as_str().into(),
        });
        self.by_oid.insert(oid, idx);
        true
    }

    pub fn insert_data(&mut self, data: CommitData, author: Author) -> bool {
        if self.by_oid.contains_key(&data.oid) {
            return false;
        }
        let author_idx = self.intern_author(&author.name, &author.email);
        let idx = self.commits.len() as u32;
        let mut d = data;
        d.author = author_idx;
        self.by_oid.insert(d.oid, idx);
        self.commits.push(d);
        true
    }

    /// For each commit index, whether it is reachable from `tips` (a tip itself counts).
    pub fn reachable_from(&self, tips: &[Oid]) -> Vec<bool> {
        let mut keep = vec![false; self.commits.len()];
        let mut stack: Vec<u32> = tips.iter().filter_map(|t| self.index_of(t)).collect();
        while let Some(i) = stack.pop() {
            if std::mem::replace(&mut keep[i as usize], true) {
                continue;
            }
            for p in &self.commits[i as usize].parents {
                if let Some(pi) = self.index_of(p) {
                    if !keep[pi as usize] {
                        stack.push(pi);
                    }
                }
            }
        }
        keep
    }

    /// Keep only commits reachable from `tips`; returns the dropped oids.
    pub fn retain_reachable_from(&mut self, tips: &[Oid]) -> Vec<Oid> {
        let mut keep = vec![false; self.commits.len()];
        let mut stack: Vec<u32> = tips.iter().filter_map(|t| self.index_of(t)).collect();
        while let Some(i) = stack.pop() {
            if keep[i as usize] {
                continue;
            }
            keep[i as usize] = true;
            for p in &self.commits[i as usize].parents {
                if let Some(pi) = self.index_of(p) {
                    if !keep[pi as usize] {
                        stack.push(pi);
                    }
                }
            }
        }
        if keep.iter().all(|k| *k) {
            return Vec::new();
        }
        let mut dropped = Vec::new();
        let old = std::mem::take(&mut self.commits);
        self.by_oid.clear();
        for (i, c) in old.into_iter().enumerate() {
            if keep[i] {
                let idx = self.commits.len() as u32;
                self.by_oid.insert(c.oid, idx);
                self.commits.push(c);
            } else {
                dropped.push(c.oid);
            }
        }
        dropped
    }
}
