use std::alloc::{AllocError, Allocator, Layout, System};
use std::collections::LinkedList;
use std::ptr::NonNull;
use std::sync::MutexGuard;

use crate::mutex::{Lock, Locked};

use crate::stats::MemStats;

pub struct SimpleSegregatedStorage {
    lists: [LinkedList<NonNull<[u8]>>; 10],
    allocated_first_byte: Vec<NonNull<u8>>,
    total_size: f64,
    peak_allocated_size: f64,
    current_allocated_size: f64,
}

impl SimpleSegregatedStorage {
    pub fn new() -> Self {
        SimpleSegregatedStorage {
            lists: [
                LinkedList::new(),
                LinkedList::new(),
                LinkedList::new(),
                LinkedList::new(),
                LinkedList::new(),
                LinkedList::new(),
                LinkedList::new(),
                LinkedList::new(),
                LinkedList::new(),
                LinkedList::new(),
            ],
            allocated_first_byte: Vec::new(),
            total_size: 0.0,
            peak_allocated_size: 0.0,
            current_allocated_size: 0.0,
        }
    }
}

impl MemStats for SimpleSegregatedStorage {
    fn calculate_allocation_ratio(&self) -> (f64, f64, f64) {
        (
            self.peak_allocated_size,
            self.total_size,
            self.peak_allocated_size / self.total_size,
        )
    }

    fn reset(&mut self) {
        self.total_size = 0.0;
        self.peak_allocated_size = 0.0;
        self.current_allocated_size = 0.0;
        for byte in &self.allocated_first_byte {
            unsafe {
                System.deallocate(*byte, Layout::from_size_align_unchecked(512, 16));
            }
        }
        self.allocated_first_byte.clear();
        for list in &mut self.lists {
            while list.pop_front().is_some() {}
        }
    }
}

impl Drop for SimpleSegregatedStorage {
    fn drop(&mut self) {
        for byte in &self.allocated_first_byte {
            unsafe {
                System.deallocate(*byte, Layout::from_size_align_unchecked(512, 16));
            }
        }
        for list in &mut self.lists {
            while list.pop_front().is_some() {}
        }
    }
}

unsafe impl Allocator for Locked<SimpleSegregatedStorage> {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        // Round up allocation to nearest power of 2. Options are 1B, 2B, 4B, 8B, 16B, 32B, 64B, 128B, 256B, 512B
        let mut alloc: MutexGuard<'_, SimpleSegregatedStorage> = self.lock();
        let mut rounded_size: usize = 1;
        let mut index: usize = 0;

        if layout.size() > 512 {
            return Err(AllocError);
        } else {
            let mut temp: usize = layout.size() - 1;
            while temp != 0 {
                temp >>= 1;
                rounded_size <<= 1;
                index += 1;
            }
        }

        unsafe {
            let modified_layout: Layout = Layout::from_size_align_unchecked(512, 16);
            if alloc.lists[index].is_empty() {
                let ptr: NonNull<[u8]> = System.allocate(modified_layout).unwrap();
                alloc
                    .allocated_first_byte
                    .push(NonNull::new_unchecked(ptr.as_mut_ptr()));
                let raw_ptr: *mut [u8] = ptr.as_ptr();
                let chunks = (*raw_ptr).chunks_exact_mut(rounded_size);
                for chunk in chunks {
                    alloc.lists[index].push_back(NonNull::new_unchecked(chunk as *mut [u8]));
                }

                // Increment total size due to new allocation
                alloc.total_size += 512.0;
            }

            // update allocation stats
            alloc.current_allocated_size += rounded_size as f64;
            alloc.peak_allocated_size =
                f64::max(alloc.current_allocated_size, alloc.peak_allocated_size);

            Ok(alloc.lists[index].pop_front().unwrap())
        }
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        let mut alloc: MutexGuard<'_, SimpleSegregatedStorage> = self.lock();
        let mut rounded_size: usize = 1;
        let mut index: usize = 0;

        if layout.size() > 512 {
            return;
        } else {
            let mut temp: usize = layout.size() - 1;
            while temp != 0 {
                temp >>= 1;
                rounded_size <<= 1;
                index += 1;
            }
        }

        // let mut vec: Vec<u8> = Vec::new();
        // for i in 0..rounded_size {
        //     vec.push(*(ptr.as_ptr().add(i)));
        // }
        // let slice: &mut [u8] = &mut vec.as_mut_slice();
        let slice: NonNull<[u8]> = NonNull::slice_from_raw_parts(ptr, layout.size());

        alloc.lists[index].push_back(slice);

        // Decrement current allocation size
        alloc.current_allocated_size -= rounded_size as f64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn test_allocate_fail() {
        let allocator: Locked<SimpleSegregatedStorage> =
            Locked::new(SimpleSegregatedStorage::new());
        let layout: Layout = Layout::from_size_align(1024, 8).unwrap();
        assert_eq!(allocator.allocate(layout), Err(AllocError));
    }

    #[test]
    fn test_allocate_deallocate_success() {
        let allocator: Locked<SimpleSegregatedStorage> =
            Locked::new(SimpleSegregatedStorage::new());
        let layout: Layout = Layout::from_size_align(128, 8).unwrap();

        // Allocate with corresponding layout
        let ptr: NonNull<[u8]> = allocator.allocate(layout).unwrap();

        // Verify blocks created correctly and allocated
        let alloc: MutexGuard<'_, SimpleSegregatedStorage> = allocator.lock();
        assert_eq!(alloc.lists[7].len(), 3); // 4 created, 3 stored while 1 is used for the allocation
        Mutex::unlock(alloc);

        unsafe {
            let raw_first_byte: *mut u8 = ptr.as_mut_ptr();
            allocator.deallocate(NonNull::new_unchecked(raw_first_byte), layout);

            // Verify deallocated block still exists and is added to correct list
            let alloc: MutexGuard<'_, SimpleSegregatedStorage> = allocator.lock();
            assert_eq!(alloc.lists[7].len(), 4) // deallocated block should be added to corresponding list
        }
    }

    #[test]
    fn test_allocation_stats() {
        let allocator: Locked<SimpleSegregatedStorage> =
            Locked::new(SimpleSegregatedStorage::new());
        let layout: Layout = Layout::from_size_align(256, 8).unwrap();
        let _ = allocator.allocate(layout).unwrap();

        let layout: Layout = Layout::from_size_align(128, 8).unwrap();
        let ptr = allocator.allocate(layout).unwrap();

        unsafe {
            let raw_first_byte: *mut u8 = ptr.as_mut_ptr();
            allocator.deallocate(NonNull::new_unchecked(raw_first_byte), layout);
        }

        let layout: Layout = Layout::from_size_align(32, 8).unwrap();
        let _ = allocator.allocate(layout).unwrap();

        let alloc: MutexGuard<'_, SimpleSegregatedStorage> = allocator.lock();
        assert_eq!(alloc.total_size, 1536 as f64);
        assert_eq!(alloc.peak_allocated_size, 384 as f64);
        assert_eq!(alloc.current_allocated_size, 288 as f64);
    }
}
