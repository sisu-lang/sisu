//! The sets of a table's variables, with a stamp for each change. A rebuilt type reads the parent
//! links and the root record of each variable that it names, and nothing else of the table. Each
//! write of these goes through `link` or `set_root`, which stamps the variable. So a memo entry of
//! a rebuilt type is good while no variable that it read has a stamp above the entry's.

use std::cell::Cell;

use super::Kind;
use crate::tir::Type;

/// What is known of the set of variables that one root stands for.
#[derive(Clone, Copy)]
pub(super) struct Root {
    /// How many variables are in the set: a union puts the smaller set under the larger.
    pub(super) size: u32,
    pub(super) kind: Kind,
    pub(super) bound: Option<Type>,
    /// A failed use reached the set: it reports nothing more.
    pub(super) poisoned: bool,
    /// `write_back` has dealt with the set.
    pub(super) reported: bool,
}

#[derive(Default)]
pub(super) struct Sets {
    /// Each variable's parent: its own id for a root. A cell, so that a find halves a path through
    /// `&self`.
    parents: Vec<Cell<u32>>,
    /// Read for a root only.
    roots: Vec<Root>,
    /// How many writes `link` and `set_root` have made.
    changes: usize,
    /// The count of changes at the last write of each variable.
    stamps: Stamps,
}

impl Sets {
    /// A new variable of `kind`, in a set of its own: its id.
    pub(super) fn push(&mut self, kind: Kind) -> u32 {
        let id = u32::try_from(self.parents.len())
            .expect("a function should make fewer than 2^32 variables");
        self.parents.push(Cell::new(id));
        self.roots.push(Root {
            size: 1,
            kind,
            bound: None,
            poisoned: false,
            reported: false,
        });
        self.stamps.push();
        id
    }

    pub(super) fn parent(&self, id: u32) -> u32 {
        self.parents[id as usize].get()
    }

    /// Moves `id` up to `to`, a variable on its path to its root: the root stays the same, so this
    /// is no change.
    pub(super) fn halve(&self, id: u32, to: u32) {
        self.parents[id as usize].set(to);
    }

    /// Puts `child` under `to`, or back on its own when `to` is `child`.
    pub(super) fn link(&mut self, child: u32, to: u32) {
        self.parents[child as usize].set(to);
        self.touch(child);
    }

    pub(super) fn root(&self, id: u32) -> Root {
        self.roots[id as usize]
    }

    pub(super) fn set_root(&mut self, id: u32, root: Root) {
        self.roots[id as usize] = root;
        self.touch(id);
    }

    /// How many changes were made: an entry made now is good while no variable that it read has
    /// a later stamp.
    pub(super) fn changes(&self) -> usize {
        self.changes
    }

    /// The latest stamp of the variables `lo` to `hi`, both included.
    pub(super) fn newest(&self, lo: u32, hi: u32) -> usize {
        self.stamps.max(lo as usize, hi as usize)
    }

    fn touch(&mut self, id: u32) {
        self.changes += 1;
        self.stamps.set(id as usize, self.changes);
    }
}

/// A max segment tree over the variables: leaf `i` at `cap + i`, and each other node the larger of
/// its two children.
#[derive(Default)]
struct Stamps {
    len: usize,
    tree: Vec<usize>,
}

impl Stamps {
    fn cap(&self) -> usize {
        self.tree.len() / 2
    }

    /// Adds a leaf with the stamp 0. The tree doubles when it is full, so a push costs O(1) on
    /// average.
    fn push(&mut self) {
        let cap = self.cap();
        if self.len == cap {
            let wide = (cap * 2).max(1);
            let mut tree = vec![0; 2 * wide];
            tree[wide..wide + cap].copy_from_slice(&self.tree[cap..]);
            for node in (1..wide).rev() {
                tree[node] = tree[2 * node].max(tree[2 * node + 1]);
            }
            self.tree = tree;
        }
        self.len += 1;
    }

    fn set(&mut self, at: usize, stamp: usize) {
        let mut node = self.cap() + at;
        self.tree[node] = stamp;
        while node > 1 {
            node /= 2;
            self.tree[node] = self.tree[2 * node].max(self.tree[2 * node + 1]);
        }
    }

    /// The largest stamp of the leaves `lo` to `hi`, both included.
    fn max(&self, lo: usize, hi: usize) -> usize {
        let cap = self.cap();
        let (mut left, mut right, mut most) = (lo + cap, hi + cap + 1, 0);
        while left < right {
            if left % 2 == 1 {
                most = most.max(self.tree[left]);
                left += 1;
            }
            if right % 2 == 1 {
                right -= 1;
                most = most.max(self.tree[right]);
            }
            left /= 2;
            right /= 2;
        }
        most
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Leaves 0 to 9, each set to a stamp, against a direct scan of each range. The tree grows
    /// at 1, 2, 4 and 8 leaves.
    #[test]
    fn max_should_give_the_largest_stamp_of_each_range() {
        let mut stamps = Stamps::default();
        let mut direct = Vec::new();
        for (i, stamp) in [5, 1, 9, 3, 7, 2, 8, 4, 6, 10].into_iter().enumerate() {
            stamps.push();
            direct.push(0);
            stamps.set(i, stamp);
            direct[i] = stamp;
            for lo in 0..=i {
                for hi in lo..=i {
                    let want = direct[lo..=hi].iter().copied().max();
                    assert_eq!(Some(stamps.max(lo, hi)), want, "leaves {lo} to {hi}");
                }
            }
        }
    }
}
