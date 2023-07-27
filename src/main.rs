#![feature(allocator_api)]
#![feature(linked_list_cursors)]
#![feature(mutex_unlock)]
#![feature(slice_ptr_get)]
#![feature(strict_provenance)]

use std::sync::{Mutex, MutexGuard};

mod buddy;
mod mutex;
mod segregated_free_list;
mod simple_segregated_storage;
mod stats;

use crate::buddy::Buddy;
use crate::mutex::{Lock, Locked};
use crate::segregated_free_list::SegregatedFreeList;
use crate::simple_segregated_storage::SimpleSegregatedStorage;
use crate::stats::MemStats;

fn main() {
    println!("\nTesting Simple Segregated Storage Allocator");
    let allocator = Locked::new(SimpleSegregatedStorage::new());
    test_throughput(&allocator);
    test_peak_memory_usage(&allocator);

    println!("\nTesting Segregated Free List Allocator");
    let allocator = Locked::new(SegregatedFreeList::new());
    test_throughput(&allocator);
    test_peak_memory_usage(&allocator);

    println!("\nTesting Buddy Allocator");
    let allocator = Locked::new(Buddy::new());
    test_throughput(&allocator);
    test_peak_memory_usage(&allocator);
}

fn test_throughput<T: std::alloc::Allocator>(allocator: &T) {
    use std::time::{Duration, Instant};
    const TOTAL: f64 = 5.0;
    let start: Instant = Instant::now();

    let _b = Box::new_in(1_u8, allocator);
    {
        let _c = Box::new_in(60_u64, allocator);
        let _d = Box::new_in(2_u8, allocator);
        let _e = Box::new_in(4_u32, allocator);
        let _f = Box::new_in(100_u64, allocator);
    }
    let _g = Box::new_in(100_u128, allocator);
    let _h = Box::new_in(100_u16, allocator);
    let _i = Box::new_in(100_u64, allocator);
    {
        let _j = Box::new_in(100_u128, allocator);
        {
            let _k = Box::new_in(100_u64, allocator);
            let _l = Box::new_in(100_u16, allocator);
        }
        let _m = Box::new_in(100_u32, allocator);
    }
    let _n = Box::new_in(100_u128, allocator);
    let _o = Box::new_in(100_u64, allocator);
    let _p = Box::new_in(100_u64, allocator);

    let end: Instant = Instant::now();
    let delta: Duration = end - start;
    println!(
        "num_allocations: {}\ntime_taken: {} seconds\nthroughput: {} allocations per seconds",
        TOTAL as usize,
        delta.as_secs_f64(),
        TOTAL / delta.as_secs_f64()
    );
}

fn test_peak_memory_usage<A: MemStats, T: std::alloc::Allocator + Lock<A>>(allocator: &T) {
    // reset stats
    let mut alloc: MutexGuard<'_, A> = allocator.lock();
    alloc.reset();
    Mutex::unlock(alloc);

    let _b = Box::new_in(1_u8, allocator);
    {
        let _c = Box::new_in(60_u128, allocator);
        let _d = Box::new_in(2_u128, allocator);
        let _e = Box::new_in(4_u128, allocator);
        let _f = Box::new_in(100_u128, allocator);
    }
    let _g = Box::new_in(100_u128, allocator);
    {
        let _j = Box::new_in(100_u128, allocator);
        {
            let _k = Box::new_in(100_u64, allocator);
            let _l = Box::new_in(100_u16, allocator);
        }
        let _m = Box::new_in(100_u32, allocator);
    }
    let _h = Box::new_in(100_u16, allocator);
    let _i = Box::new_in(100_u64, allocator);
    let _n = Box::new_in(100_u128, allocator);
    let _o = Box::new_in(100_u64, allocator);
    let _p = Box::new_in(100_u64, allocator);

    let alloc: MutexGuard<'_, A> = allocator.lock();
    let (allocated_size, total_size, peak_mem_usage_ratio): (f64, f64, f64) =
        (*alloc).calculate_allocation_ratio();
    println!(
        "allocated_memory: {} bytes\ntotal_memory: {} bytes\npeak_memory_usage_ratio {} ",
        allocated_size, total_size, peak_mem_usage_ratio
    );
}
