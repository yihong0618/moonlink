use more_asserts as ma;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Batch ID counter for the two-counter allocation strategy.
///
/// The system uses two separate atomic counters to partition the 64-bit batch ID space:
/// - **Streaming Counter**: Range 0 -> 2^63-1, used for streaming transactions
/// - **Non-Streaming Counter**: Range 2^63+, used for regular operations
///
/// We give streaming batches the smaller range so that they are always behind the commit point, which points to the most recently added batch of the non-streaming batches.
/// This ensures batch IDs are always monotonically increasing and unique across all transactions.
pub(super) struct BatchIdCounter {
    counter: Arc<AtomicU64>,
    is_streaming: bool,
}

impl BatchIdCounter {
    pub fn new(is_streaming: bool) -> Self {
        Self {
            counter: Arc::new(AtomicU64::new(if is_streaming { 0 } else { 1u64 << 63 })),
            is_streaming,
        }
    }

    // Relaxed ordering is used here because the counter is only used for internal state tracking, not for synchronization.
    pub fn load(&self) -> u64 {
        self.counter.load(Ordering::Relaxed)
    }

    // Relaxed ordering is used here because the counter is only used for internal state tracking, not for synchronization.
    pub fn next(&self) -> u64 {
        let current = self.counter.load(Ordering::Relaxed);

        // Check limits before incrementing
        if self.is_streaming {
            ma::assert_lt!(
                current,
                (1u64 << 63),
                "Streaming batch ID counter overflow: exceeded 2^63-1"
            );
        } else {
            ma::assert_lt!(current, u64::MAX, "Non-streaming batch ID counter overflow");
        }

        self.counter.fetch_add(1, Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_streaming_counter_creation() {
        let counter = BatchIdCounter::new(true);
        assert_eq!(counter.load(), 0);
        assert!(counter.is_streaming);
    }

    #[test]
    fn test_non_streaming_counter_creation() {
        let counter = BatchIdCounter::new(false);
        assert_eq!(counter.load(), 1u64 << 63);
        assert!(!counter.is_streaming);
    }

    #[test]
    fn test_streaming_counter_next() {
        let counter = BatchIdCounter::new(true);

        // First call should return 0, then increment to 1
        assert_eq!(counter.next(), 0);
        assert_eq!(counter.load(), 1);

        // Second call should return 1, then increment to 2
        assert_eq!(counter.next(), 1);
        assert_eq!(counter.load(), 2);
    }

    #[test]
    fn test_non_streaming_counter_next() {
        let counter = BatchIdCounter::new(false);
        let expected_start = 1u64 << 63;

        // First call should return 2^63, then increment to 2^63 + 1
        assert_eq!(counter.next(), expected_start);
        assert_eq!(counter.load(), expected_start + 1);

        // Second call should return 2^63 + 1, then increment to 2^63 + 2
        assert_eq!(counter.next(), expected_start + 1);
        assert_eq!(counter.load(), expected_start + 2);
    }

    #[test]
    #[should_panic(expected = "Streaming batch ID counter overflow: exceeded 2^63-1")]
    fn test_streaming_counter_overflow() {
        let counter = BatchIdCounter::new(true);

        // Manually set counter to the limit
        let limit = 1u64 << 63;
        counter.counter.store(limit, Ordering::Relaxed);

        // This should panic
        counter.next();
    }

    #[test]
    #[should_panic(expected = "Non-streaming batch ID counter overflow")]
    fn test_non_streaming_counter_overflow() {
        let counter = BatchIdCounter::new(false);

        // Manually set counter to u64::MAX
        counter.counter.store(u64::MAX, Ordering::Relaxed);

        // This should panic
        counter.next();
    }

    #[test]
    fn test_streaming_counter_near_limit() {
        let counter = BatchIdCounter::new(true);
        let near_limit = (1u64 << 63) - 2;

        // Set counter near the limit
        counter.counter.store(near_limit, Ordering::Relaxed);

        // These should work
        assert_eq!(counter.next(), near_limit);
        assert_eq!(counter.next(), near_limit + 1);

        // The next call should panic - test this separately to ensure it panics
    }

    #[test]
    fn test_concurrent_access() {
        let counter = Arc::new(BatchIdCounter::new(true));
        let num_threads = 10;
        let increments_per_thread = 100;

        let handles: Vec<_> = (0..num_threads)
            .map(|_| {
                let counter_clone = Arc::clone(&counter);
                thread::spawn(move || {
                    let mut ids = Vec::new();
                    for _ in 0..increments_per_thread {
                        ids.push(counter_clone.next());
                    }
                    ids
                })
            })
            .collect();

        // Collect all IDs from all threads
        let mut all_ids = Vec::new();
        for handle in handles {
            all_ids.extend(handle.join().unwrap());
        }

        // All IDs should be unique
        all_ids.sort_unstable();
        let mut unique_ids = all_ids.clone();
        unique_ids.dedup();

        assert_eq!(all_ids.len(), unique_ids.len(), "All IDs should be unique");
        assert_eq!(all_ids.len(), num_threads * increments_per_thread);

        // All IDs should be in streaming range
        for id in &all_ids {
            assert!(*id < (1u64 << 63), "ID {id} should be in streaming range");
        }

        // IDs should be consecutive starting from 0
        for (i, &id) in all_ids.iter().enumerate() {
            assert_eq!(id, i as u64, "IDs should be consecutive");
        }
    }
}
