use std::alloc::{AllocError, Allocator, Layout, System};
use std::collections::linked_list::CursorMut;
use std::collections::LinkedList;
use std::ptr::NonNull;
use std::sync::MutexGuard;

use crate::mutex::{Lock, Locked};
use crate::stats::MemStats;

// Holds 10 fixed size lists of sizes 1,2,4,8,16,32,64,128,256,512
pub struct Buddy {
    lists: [LinkedList<NonNull<[u8]>>; 10],
    first_byte_ptrs: Vec<NonNull<u8>>,
    total_size: f64,
    peak_allocated_size: f64,
    current_allocated_size: f64,
}

impl Buddy {
    pub fn new() -> Self {
        Buddy {
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
            first_byte_ptrs: Vec::new(),
            total_size: 0.0,
            peak_allocated_size: 0.0,
            current_allocated_size: 0.0,
        }
    }
}

impl Drop for Buddy {
    fn drop(&mut self) {
        let extend_heap_layout: Layout = Layout::from_size_align(512, 16).unwrap();
        unsafe {
            for ptr in &self.first_byte_ptrs {
                System.deallocate(*ptr, extend_heap_layout);
            }
        }
    }
}

impl MemStats for Buddy {
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
        for byte in &self.first_byte_ptrs {
            unsafe {
                System.deallocate(*byte, Layout::from_size_align_unchecked(512, 16));
            }
        }
        self.first_byte_ptrs.clear();
        for list in &mut self.lists {
            while list.pop_front().is_some() {}
        }
    }
}

unsafe impl Allocator for Locked<Buddy> {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        // round up to the nearest power of 2 for allocation
        let requested_size: usize = layout.size();
        let mut rounded_size: usize = 1;
        let mut index: usize = 0;

        // we will assume 512 is the max request size
        if requested_size > 512 {
            return Err(AllocError);
        } else {
            let mut curr_power: usize = requested_size - 1;
            while curr_power != 0 {
                curr_power >>= 1;
                rounded_size <<= 1;
                index += 1;
            }
        }

        // now we check if we already have a block available to allocate
        let mut alloc_mutex: MutexGuard<'_, Buddy> = self.lock();
        let mut find_index: usize = index;

        while find_index < 10 {
            if alloc_mutex.lists[find_index].is_empty() {
                find_index += 1;
            } else {
                break;
            }
        }

        // if no block found, extend the heap
        if find_index >= 10 {
            // need to extend heap
            let extend_heap_layout: Layout = Layout::from_size_align(512, 16).unwrap();
            let ptr: NonNull<[u8]> = System.allocate(extend_heap_layout).unwrap();
            // ln!("{}", ptr.addr());
            let first_byte_ptr: NonNull<u8> = ptr.as_non_null_ptr();
            alloc_mutex.lists[9].push_back(ptr);
            alloc_mutex.first_byte_ptrs.push(first_byte_ptr);
            // println!("{:#?}", alloc_mutex.first_byte_ptrs)
            alloc_mutex.total_size += 512.0;
        }

        // recursively split block until we have one that fits the size we want (rounded size)
        find_index = index + 1;
        let mut allocated_block: Option<NonNull<[u8]>> = None;

        while allocated_block.is_none() {
            match alloc_mutex.lists[index].pop_front() {
                Some(block) => {
                    allocated_block = Some(block);
                }
                None => match alloc_mutex.lists[find_index].pop_front() {
                    None => {
                        find_index += 1;
                    }
                    Some(mut unsplit_block) => unsafe {
                        find_index -= 1;
                        let unsplit_block_mut: &mut [u8] = unsplit_block.as_mut();
                        let split_len: usize = unsplit_block_mut.len() >> 1;
                        let (block_one, block_two): (&mut [u8], &mut [u8]) =
                            unsplit_block_mut.split_at_mut(split_len);
                        alloc_mutex.lists[find_index].push_back(NonNull::slice_from_raw_parts(
                            NonNull::new(block_one.as_mut_ptr()).unwrap(),
                            split_len,
                        ));
                        alloc_mutex.lists[find_index].push_back(NonNull::slice_from_raw_parts(
                            NonNull::new(block_two.as_mut_ptr()).unwrap(),
                            split_len,
                        ));
                    },
                },
            }
        }
        alloc_mutex.current_allocated_size += rounded_size as f64;
        alloc_mutex.peak_allocated_size = f64::max(
            alloc_mutex.current_allocated_size,
            alloc_mutex.peak_allocated_size,
        );

        // guaranteed to contain a block
        Ok(allocated_block.unwrap())
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        let requested_size: usize = layout.size();
        let mut curr_ptr = ptr;

        let mut alloc_mutex = self.lock();
        let offset: usize = alloc_mutex.first_byte_ptrs[0].addr().get();

        let mut rounded_size: usize = 1;
        let mut curr_power: usize = requested_size - 1;
        let mut index = 0;

        while curr_power != 0 {
            curr_power >>= 1;
            rounded_size <<= 1;
            index += 1;
        }

        alloc_mutex.current_allocated_size -= rounded_size as f64;
        loop {
            if rounded_size == 512 {
                let slice_ptr: NonNull<[u8]> =
                    NonNull::slice_from_raw_parts(curr_ptr, rounded_size);
                alloc_mutex.lists[9].push_back(slice_ptr);
                return;
            }

            let current_addr: usize = curr_ptr.addr().get();
            let normalized_addr: usize = current_addr - offset; // should always be positive since offset is first address

            // get address of buddy (or if we have the smaller of the pair, xor if we have the larger of the pair)
            let mut normalized_buddy_address: usize = normalized_addr | rounded_size;
            if normalized_buddy_address == normalized_addr {
                normalized_buddy_address = normalized_addr ^ rounded_size;
            }

            let buddy_address: usize = normalized_buddy_address + offset;

            let mut buddy: Option<NonNull<[u8]>> = None;
            let mut cursor: CursorMut<'_, NonNull<[u8]>> =
                alloc_mutex.lists[index].cursor_front_mut();
            while buddy.is_none() && cursor.current().is_some() {
                let curr = cursor.current().unwrap();
                if buddy_address == curr.addr().get() {
                    buddy = cursor.remove_current();
                }
                cursor.move_next();
            }

            if buddy.is_none() {
                let slice_ptr: NonNull<[u8]> =
                    NonNull::slice_from_raw_parts(curr_ptr, rounded_size);
                alloc_mutex.lists[index].push_back(slice_ptr);
                return;
            }

            rounded_size <<= 1;
            index += 1;
            if current_addr > buddy_address {
                curr_ptr = buddy.unwrap().as_non_null_ptr();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn test_allocate_fail() {
        let allocator: Locked<Buddy> = Locked::new(Buddy::new());
        let invalid_layout: Layout = Layout::from_size_align(1024, 16).unwrap();
        assert_eq!(allocator.allocate(invalid_layout), Err(AllocError));
    }

    #[test]
    fn test_allocate_success() {
        let allocator: Locked<Buddy> = Locked::new(Buddy::new());
        let layout: Layout = Layout::from_size_align(120, 8).unwrap();
        let ptr: Result<NonNull<[u8]>, AllocError> = allocator.allocate(layout);

        assert!(ptr.is_ok());
        assert_eq!(ptr.unwrap().len(), 128);

        // verify blocks are split correctly
        // should have one 256 block and one 128 block (index 7 and 8)
        let alloc_mutex: MutexGuard<'_, Buddy> = allocator.lock();
        assert_eq!(alloc_mutex.lists[7].len(), 1);
        assert_eq!(alloc_mutex.lists[8].len(), 1);
        Mutex::unlock(alloc_mutex);

        // Allocate exactly size of list
        let layout: Layout = Layout::from_size_align(32, 8).unwrap();
        let ptr: Result<NonNull<[u8]>, AllocError> = allocator.allocate(layout);

        assert!(ptr.is_ok());
        assert_eq!(ptr.unwrap().len(), 32);

        // should now have one 256 block, one 64 block, and one 32 block (index 5, 6, 8)
        let alloc_mutex: MutexGuard<'_, Buddy> = allocator.lock();
        assert_eq!(alloc_mutex.lists[5].len(), 1);
        assert_eq!(alloc_mutex.lists[6].len(), 1);
        assert_eq!(alloc_mutex.lists[7].len(), 0);
        assert_eq!(alloc_mutex.lists[8].len(), 1);
        Mutex::unlock(alloc_mutex);
    }

    #[test]
    fn test_deallocate_success() {
        // TODO: Need to change recursion to a loop to avoid stack overflows + increase performance gains
        let allocator: Locked<Buddy> = Locked::new(Buddy::new());
        let layout: Layout = Layout::from_size_align(120, 8).unwrap();
        let ptr: NonNull<[u8]> = allocator.allocate(layout.clone()).unwrap();

        unsafe {
            let first_byte_ptr: NonNull<u8> = ptr.as_non_null_ptr();
            allocator.deallocate(first_byte_ptr, layout)
        }
        let alloc_mutex: MutexGuard<'_, Buddy> = allocator.lock();
        assert_eq!(alloc_mutex.lists[0].len(), 0);
        assert_eq!(alloc_mutex.lists[1].len(), 0);
        assert_eq!(alloc_mutex.lists[2].len(), 0);
        assert_eq!(alloc_mutex.lists[3].len(), 0);
        assert_eq!(alloc_mutex.lists[4].len(), 0);
        assert_eq!(alloc_mutex.lists[5].len(), 0);
        assert_eq!(alloc_mutex.lists[6].len(), 0);
        assert_eq!(alloc_mutex.lists[7].len(), 0);
        assert_eq!(alloc_mutex.lists[8].len(), 0);
        assert_eq!(alloc_mutex.lists[9].len(), 1);
        Mutex::unlock(alloc_mutex);

        let ptr = allocator.allocate(layout.clone()).unwrap();
        let alloc_mutex: MutexGuard<'_, Buddy> = allocator.lock();
        // println!("{:#?}", alloc_mutex.lists);
        assert_eq!(alloc_mutex.lists[0].len(), 0);
        assert_eq!(alloc_mutex.lists[1].len(), 0);
        assert_eq!(alloc_mutex.lists[2].len(), 0);
        assert_eq!(alloc_mutex.lists[3].len(), 0);
        assert_eq!(alloc_mutex.lists[4].len(), 0);
        assert_eq!(alloc_mutex.lists[5].len(), 0);
        assert_eq!(alloc_mutex.lists[6].len(), 0);
        assert_eq!(alloc_mutex.lists[7].len(), 1);
        assert_eq!(alloc_mutex.lists[8].len(), 1);
        assert_eq!(alloc_mutex.lists[9].len(), 0);
        Mutex::unlock(alloc_mutex);

        let smaller_layout: Layout = Layout::from_size_align(3, 8).unwrap();
        let ptr2: NonNull<[u8]> = allocator.allocate(smaller_layout.clone()).unwrap();

        let alloc_mutex: MutexGuard<'_, Buddy> = allocator.lock();
        // println!("{:#?}", alloc_mutex.lists);
        assert_eq!(alloc_mutex.lists[0].len(), 0);
        assert_eq!(alloc_mutex.lists[1].len(), 0);
        assert_eq!(alloc_mutex.lists[2].len(), 1);
        assert_eq!(alloc_mutex.lists[3].len(), 1);
        assert_eq!(alloc_mutex.lists[4].len(), 1);
        assert_eq!(alloc_mutex.lists[5].len(), 1);
        assert_eq!(alloc_mutex.lists[6].len(), 1);
        assert_eq!(alloc_mutex.lists[7].len(), 0);
        assert_eq!(alloc_mutex.lists[8].len(), 1);
        assert_eq!(alloc_mutex.lists[9].len(), 0);
        Mutex::unlock(alloc_mutex);

        unsafe {
            let first_byte_ptr: NonNull<u8> = ptr.as_non_null_ptr();
            allocator.deallocate(first_byte_ptr, layout);
        }
        let alloc_mutex: MutexGuard<'_, Buddy> = allocator.lock();
        // println!("{:#?}", alloc_mutex.lists);
        assert_eq!(alloc_mutex.lists[0].len(), 0);
        assert_eq!(alloc_mutex.lists[1].len(), 0);
        assert_eq!(alloc_mutex.lists[2].len(), 1);
        assert_eq!(alloc_mutex.lists[3].len(), 1);
        assert_eq!(alloc_mutex.lists[4].len(), 1);
        assert_eq!(alloc_mutex.lists[5].len(), 1);
        assert_eq!(alloc_mutex.lists[6].len(), 1);
        assert_eq!(alloc_mutex.lists[7].len(), 1);
        assert_eq!(alloc_mutex.lists[8].len(), 1);
        assert_eq!(alloc_mutex.lists[9].len(), 0);
        Mutex::unlock(alloc_mutex);

        unsafe {
            let first_byte_ptr: NonNull<u8> = ptr2.as_non_null_ptr();
            allocator.deallocate(first_byte_ptr, smaller_layout);
        }
        let alloc_mutex: MutexGuard<'_, Buddy> = allocator.lock();
        // println!("{:#?}", alloc_mutex.lists);
        assert_eq!(alloc_mutex.lists[0].len(), 0);
        assert_eq!(alloc_mutex.lists[1].len(), 0);
        assert_eq!(alloc_mutex.lists[2].len(), 0);
        assert_eq!(alloc_mutex.lists[3].len(), 0);
        assert_eq!(alloc_mutex.lists[4].len(), 0);
        assert_eq!(alloc_mutex.lists[5].len(), 0);
        assert_eq!(alloc_mutex.lists[6].len(), 0);
        assert_eq!(alloc_mutex.lists[7].len(), 0);
        assert_eq!(alloc_mutex.lists[8].len(), 0);
        assert_eq!(alloc_mutex.lists[9].len(), 1);
        Mutex::unlock(alloc_mutex);
    }

    #[test]
    fn test_allocation_stats() {
        let allocator: Locked<Buddy> = Locked::new(Buddy::new());
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

        let alloc: MutexGuard<'_, Buddy> = allocator.lock();
        assert_eq!(alloc.total_size, 512 as f64);
        assert_eq!(alloc.peak_allocated_size, 384 as f64);
        assert_eq!(alloc.current_allocated_size, 288 as f64);
    }
}
