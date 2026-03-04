// Copyright 2019 TiKV Project Authors. Licensed under Apache-2.0.
//
// Tests for Collector::clear(), HashCounter::clear(), and TempFdArray::clear().
// Included via `include!` inside `mod tests` in collector.rs so that crate-private
// types (HashCounter, TempFdArray, Collector) are accessible.

#[test]
fn hash_counter_clear() {
    let mut counter = HashCounter::<usize>::default();
    counter.add(1, 1);
    counter.add(2, 3);

    counter.clear();

    // after clear, iteration should yield nothing
    assert_eq!(counter.iter().count(), 0);

    // and new entries should work normally after clear
    counter.add(42, 7);
    let entries: Vec<_> = counter.iter().filter(|e| e.count > 0).collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].item, 42);
    assert_eq!(entries[0].count, 7);
}

#[test]
fn temp_fd_array_clear() {
    // Entry<usize> size: usize (item) + isize (count) = 16 bytes on 64-bit
    // BUFFER_LENGTH for Entry<usize> = (1<<18) / 16 = 16384
    // Push enough entries to trigger at least one flush to disk, then clear.
    let buf_len = (1 << 18) / std::mem::size_of::<Entry<usize>>();
    let mut arr = TempFdArray::<Entry<usize>>::new().unwrap();

    // Fill beyond in-memory buffer to force a flush to disk
    for i in 0..=(buf_len + 10) {
        arr.push(Entry { item: i, count: 1 }).unwrap();
    }
    assert!(arr.flush_n > 0, "expected at least one flush to disk");

    arr.clear().unwrap();

    assert_eq!(arr.buffer_index, 0);
    assert_eq!(arr.flush_n, 0);
    // After clear, the file should be empty — try_iter returns nothing from disk
    let items: Vec<_> = arr.try_iter().unwrap().collect();
    assert_eq!(items.len(), 0);

    // Should be usable again after clear
    arr.push(Entry { item: 99, count: 5 }).unwrap();
    let items: Vec<_> = arr.try_iter().unwrap().collect();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].item, 99);
}

#[test]
fn collector_clear_with_disk_eviction() {
    // Use enough distinct keys to saturate the HashCounter (4096 buckets × 4 associativity
    // = 16384 slots) and force many evictions into TempFdArray, then verify clear works.
    let mut collector = Collector::<usize>::new().unwrap();

    // Add enough distinct items to guarantee evictions into TempFdArray.
    // With 4096 buckets × 4 slots each = 16384 total slots, adding 4 × that
    // many distinct keys ensures heavy eviction pressure.
    let n = BUCKETS * BUCKETS_ASSOCIATIVITY * 4;
    for item in 0..n {
        collector.add(item, 1).unwrap();
    }

    // Sanity check: collector has data
    let total_before: isize = collector.try_iter().unwrap().map(|e| e.count).sum();
    assert!(total_before > 0);

    collector.clear().unwrap();

    // After clear, iteration must yield nothing
    let total_after: isize = collector.try_iter().unwrap().map(|e| e.count).sum();
    assert_eq!(total_after, 0);

    // Must be usable again: add fresh data and verify it's correct
    for item in 0..10 {
        collector.add(item, 2).unwrap();
    }
    let mut real_map = BTreeMap::new();
    collector.try_iter().unwrap().for_each(|e| {
        test_utils::add_map(&mut real_map, e);
    });
    for item in 0..10usize {
        assert_eq!(*real_map.get(&item).unwrap(), 2);
    }
}
