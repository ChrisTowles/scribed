//! The [rolling buffer]: a FIFO of `f32` audio frames capped at a maximum
//! length. Older frames are dropped from the front when new ones arrive past
//! capacity.
//!
//! Backed by `VecDeque<f32>` rather than the `ringbuf` crate because we need
//! arbitrary-length `extend` and we don't need lockless SPSC discipline — the
//! buffer is owned by the single inference thread.
//!
//! [rolling buffer]: ../DOMAIN.md

use std::collections::VecDeque;

#[derive(Debug)]
pub struct RollingBuffer {
    inner: VecDeque<f32>,
    capacity: usize,
}

impl RollingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.inner.len() >= self.capacity
    }

    /// Append `samples`. If the buffer would exceed capacity, drop the oldest
    /// frames from the front to make room.
    pub fn push(&mut self, samples: &[f32]) {
        self.inner.extend(samples.iter().copied());
        let overflow = self.inner.len().saturating_sub(self.capacity);
        if overflow > 0 {
            self.inner.drain(..overflow);
        }
    }

    /// Drop all frames.
    pub fn clear(&mut self) {
        self.inner.clear();
    }

    /// Copy contents into a contiguous `Vec<f32>`. Used by engines that need
    /// to pass the buffer to a tensor.
    pub fn to_vec(&self) -> Vec<f32> {
        self.inner.iter().copied().collect()
    }

    /// View as two contiguous slices (FIFO front, FIFO back). The caller can
    /// avoid the `to_vec` allocation if the engine accepts split inputs.
    pub fn as_slices(&self) -> (&[f32], &[f32]) {
        self.inner.as_slices()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_within_capacity() {
        let mut b = RollingBuffer::new(8);
        b.push(&[1.0, 2.0, 3.0]);
        assert_eq!(b.len(), 3);
        assert!(!b.is_full());
        assert_eq!(b.to_vec(), vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn push_exceeds_capacity_drops_oldest() {
        let mut b = RollingBuffer::new(4);
        b.push(&[1.0, 2.0, 3.0]);
        b.push(&[4.0, 5.0, 6.0]); // total would be 6, capacity 4
        assert_eq!(b.len(), 4);
        assert_eq!(b.to_vec(), vec![3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn single_push_larger_than_capacity_keeps_tail() {
        let mut b = RollingBuffer::new(3);
        b.push(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(b.to_vec(), vec![3.0, 4.0, 5.0]);
    }

    #[test]
    fn clear_empties() {
        let mut b = RollingBuffer::new(4);
        b.push(&[1.0, 2.0]);
        b.clear();
        assert!(b.is_empty());
    }

    #[test]
    fn empty_push_is_a_no_op() {
        let mut b = RollingBuffer::new(4);
        b.push(&[]);
        assert!(b.is_empty());
    }
}
