// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
// Third-party portions retain their terms; see THIRD_PARTY_NOTICES.md.

//! Arena-backed floating-priority Fibonacci heap used by route search.
//!
//! Index handles replace pointer links while retaining consolidation,
//! decrease-key, cascading cuts, and equal-priority ownership behavior.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Handle(usize);

#[derive(Clone, Debug)]
struct Entry<T> {
    value: T,
    priority: f64,
    degree: usize,
    marked: bool,
    next: usize,
    prev: usize,
    parent: Option<usize>,
    child: Option<usize>,
}

/// Arena-backed translation of TALA's recovered `FloatingFibonacciHeap`.
///
/// Indices replace Go pointers, but list splicing, equal-priority ownership,
/// consolidation, and cascading cuts follow `fibheap_pq.go` literally.
pub(super) struct FloatingFibonacciHeap<T> {
    entries: Vec<Entry<T>>,
    min: Option<usize>,
    size: usize,
    ring_scratch: Vec<usize>,
    trees_scratch: Vec<Option<usize>>,
}

impl<T: Copy> FloatingFibonacciHeap<T> {
    pub(super) fn new() -> Self {
        Self {
            entries: Vec::new(),
            min: None,
            size: 0,
            ring_scratch: Vec::new(),
            trees_scratch: Vec::with_capacity(16),
        }
    }

    pub(super) fn enqueue(&mut self, priority: f64, value: T) -> Handle {
        let index = self.entries.len();
        self.entries.push(Entry {
            value,
            priority,
            degree: 0,
            marked: false,
            next: index,
            prev: index,
            parent: None,
            child: None,
        });
        self.min = self.merge_lists(self.min, Some(index));
        self.size += 1;
        Handle(index)
    }

    pub(super) fn decrease_key(&mut self, handle: Handle, priority: f64) {
        let node = handle.0;
        debug_assert!(priority < self.entries[node].priority);
        self.entries[node].priority = priority;
        if self.entries[node]
            .parent
            .is_some_and(|parent| self.entries[parent].priority >= priority)
        {
            self.cut_node(node);
        }
        if self
            .min
            .is_none_or(|minimum| self.entries[node].priority <= self.entries[minimum].priority)
        {
            self.min = Some(node);
        }
    }

    pub(super) fn dequeue_min(&mut self) -> Option<(T, f64)> {
        let minimum = self.min?;
        self.size -= 1;
        if self.entries[minimum].next == minimum {
            self.min = None;
        } else {
            // Keep the field reloads between writes. TALA's pointer-based Go
            // implementation spells these mutations sequentially, which is
            // observable when a retained entry aliases a ring that has since
            // been consolidated.
            let previous = self.entries[minimum].prev;
            self.entries[previous].next = self.entries[minimum].next;
            let next = self.entries[minimum].next;
            self.entries[next].prev = self.entries[minimum].prev;
            self.min = Some(self.entries[minimum].next);
        }

        if let Some(child) = self.entries[minimum].child {
            let child_ring = self.take_ring(child);
            for current in child_ring.iter().copied() {
                self.entries[current].parent = None;
            }
            self.ring_scratch = child_ring;
        }
        self.min = self.merge_lists(self.min, self.entries[minimum].child);
        if let Some(minimum_root) = self.min {
            let roots = self.take_ring(minimum_root);
            let mut trees = std::mem::take(&mut self.trees_scratch);
            trees.clear();
            for mut current in roots.iter().copied() {
                loop {
                    let degree = self.entries[current].degree;
                    while degree >= trees.len() {
                        trees.push(None);
                    }
                    let Some(other) = trees[degree].take() else {
                        trees[degree] = Some(current);
                        break;
                    };
                    let (minimum_tree, maximum_tree) =
                        if self.entries[other].priority < self.entries[current].priority {
                            (other, current)
                        } else {
                            (current, other)
                        };
                    self.detach(maximum_tree);
                    self.entries[maximum_tree].prev = maximum_tree;
                    self.entries[maximum_tree].next = maximum_tree;
                    self.entries[minimum_tree].child =
                        self.merge_lists(self.entries[minimum_tree].child, Some(maximum_tree));
                    self.entries[maximum_tree].parent = Some(minimum_tree);
                    self.entries[maximum_tree].marked = false;
                    self.entries[minimum_tree].degree += 1;
                    current = minimum_tree;
                }
                if self.min.is_none_or(|minimum| {
                    self.entries[current].priority <= self.entries[minimum].priority
                }) {
                    self.min = Some(current);
                }
            }
            self.ring_scratch = roots;
            self.trees_scratch = trees;
        }
        Some((self.entries[minimum].value, self.entries[minimum].priority))
    }

    fn take_ring(&mut self, first: usize) -> Vec<usize> {
        let mut result = std::mem::take(&mut self.ring_scratch);
        result.clear();
        result.push(first);
        let mut current = self.entries[first].next;
        while current != first {
            result.push(current);
            current = self.entries[current].next;
        }
        result
    }

    fn detach(&mut self, node: usize) {
        let next = self.entries[node].next;
        self.entries[next].prev = self.entries[node].prev;
        let previous = self.entries[node].prev;
        self.entries[previous].next = self.entries[node].next;
    }

    fn merge_lists(&mut self, one: Option<usize>, two: Option<usize>) -> Option<usize> {
        let (Some(one), Some(two)) = (one, two) else {
            return one.or(two);
        };
        let one_next = self.entries[one].next;
        self.entries[one].next = self.entries[two].next;
        let one_new_next = self.entries[one].next;
        self.entries[one_new_next].prev = one;
        self.entries[two].next = one_next;
        let two_new_next = self.entries[two].next;
        self.entries[two_new_next].prev = two;
        if self.entries[one].priority < self.entries[two].priority {
            Some(one)
        } else {
            Some(two)
        }
    }

    fn cut_node(&mut self, node: usize) {
        self.entries[node].marked = false;
        let Some(parent) = self.entries[node].parent else {
            return;
        };
        if self.entries[node].next != node {
            self.detach(node);
        }
        if self.entries[parent].child == Some(node) {
            self.entries[parent].child = if self.entries[node].next != node {
                Some(self.entries[node].next)
            } else {
                None
            };
        }
        self.entries[parent].degree -= 1;
        self.entries[node].prev = node;
        self.entries[node].next = node;
        self.min = self.merge_lists(self.min, Some(node));
        if self.entries[parent].marked {
            self.cut_node(parent);
        } else {
            self.entries[parent].marked = true;
        }
        self.entries[node].parent = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_enqueue_replaces_the_minimum() {
        let mut heap = FloatingFibonacciHeap::new();
        heap.enqueue(1.0, 1);
        heap.enqueue(1.0, 2);
        assert_eq!(heap.dequeue_min(), Some((2, 1.0)));
    }

    #[test]
    fn decrease_key_uses_the_recovered_equal_parent_cut() {
        let mut heap = FloatingFibonacciHeap::new();
        heap.enqueue(3.0, 3);
        let lowered = heap.enqueue(4.0, 4);
        heap.enqueue(1.0, 1);
        assert_eq!(heap.dequeue_min(), Some((1, 1.0)));
        heap.decrease_key(lowered, 2.0);
        assert_eq!(heap.dequeue_min(), Some((4, 2.0)));
        assert_eq!(heap.dequeue_min(), Some((3, 3.0)));
    }
}
