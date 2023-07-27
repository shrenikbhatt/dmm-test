use std::alloc::{AllocError, Allocator, Layout, System};
use std::collections::linked_list::CursorMut;
use std::collections::LinkedList;
use std::ptr::NonNull;
use std::sync::MutexGuard;

use crate::mutex::{Lock, Locked};
use crate::stats::MemStats;

/*
    Segregated Free List Ranges (Bytes):
    - (0,32]
    - (32,64]
    - (64,128]
    - (128,256]
    - (256,MAX_ALLOWED]
    * MAX_ALLOWED is arbitrary but can keep it at 512 for now, aligned at 16.

    Allocations:
    - First fit method.
        - Round up allocation request to get corresponding list
        - Go through list to see if block available
            - If found, split to request size and place remaining in correct list
            - If not found, move to next list until all lists exhausted. (only need to check first element of following lists due to first fit)
        - If still not found, allocate block of largest size and split to request size, placing remaining in corresponding list.

    Deallocations:
    - Add freed block to corresponding list
    - Go through all values to see if there are any smaller or larger blocks that are connected to current blocks start/end address
        - If yes, connect the blocks together and place resulting block in corresponding list
    * Can also offer deferred coalescing where each freed block is placed on a queue and on the following allocations when going through queue,
      can also check if block can be coalesced. This will trade off speed for external fragmentation


*/

pub struct SegregatedFreeList {
    lists: [LinkedList<NonNull<[u8]>>; 5],
    allocated_first_byte: Vec<NonNull<u8>>,
    total_size: f64,
    peak_allocated_size: f64,
    current_allocated_size: f64,
}

impl SegregatedFreeList {
    pub fn new() -> Self {
        SegregatedFreeList {
            lists: [
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

impl Drop for SegregatedFreeList {
    fn drop(&mut self) {
        for byte in &self.allocated_first_byte {
            unsafe {
                System.deallocate(*byte, Layout::from_size_align_unchecked(512, 16));
            }
        }
    }
}

impl MemStats for SegregatedFreeList {
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

unsafe impl Allocator for Locked<SegregatedFreeList> {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let mut rounded_size: usize = 1;
        let mut index: usize = 0;
        let mut alloc: MutexGuard<'_, SegregatedFreeList> = self.lock();

        if layout.size() > 512 {
            return Err(AllocError);
        } else {
            let mut temp: usize = layout.size() - 1;
            while temp != 0 {
                temp >>= 1;
                rounded_size <<= 1;
                if rounded_size > 32 && index < 4 {
                    index += 1;
                }
            }
        }

        // Go through corresponding and following lists
        let mut allocated_node: Option<NonNull<[u8]>> = None;
        while index < 5 && allocated_node.is_none() {
            if !alloc.lists[index].is_empty() {
                let mut cursor: CursorMut<'_, NonNull<[u8]>> =
                    alloc.lists[index].cursor_front_mut();
                while cursor.current().is_some() {
                    // check size of space vs size needed
                    let ptr = cursor.current().unwrap();
                    if layout.size() <= ptr.len() {
                        allocated_node = cursor.remove_current();
                        break;
                    }
                    cursor.move_next();
                }
            }
            index += 1;
        }

        if allocated_node.is_none() {
            // need to expand heap
            unsafe {
                let modified_layout: Layout = Layout::from_size_align_unchecked(512, 16);
                let ptr: NonNull<[u8]> = System.allocate(modified_layout).unwrap();
                alloc
                    .allocated_first_byte
                    .push(NonNull::new_unchecked(ptr.as_mut_ptr()));
                allocated_node = Some(ptr);
                alloc.total_size += 512.0;
            }
        }

        // Allocate exact size needed to minimize internal fragmentation
        unsafe {
            let raw_ptr: &[u8] = allocated_node.unwrap().as_ref();
            // let s: &[u8] = & *raw_ptr;
            let (allocated, remaining): (&[u8], &[u8]) = (raw_ptr).split_at(layout.size());
            // println!("{} {}", allocated.len(), remaining.len());
            let ret: NonNull<[u8]> = NonNull::new_unchecked(allocated as *const [u8] as *mut [u8]);

            // Store remaining in corresponding list for future use
            let remaining_size: usize = remaining.len();
            // println!("{}", remaining_size);
            rounded_size = 1;
            index = 0;
            if remaining_size > 0 {
                let mut temp: usize = remaining_size - 1;
                while temp != 0 {
                    // println!("{} {} {} ", temp, rounded_size, index);
                    temp >>= 1;
                    rounded_size <<= 1;
                    if rounded_size > 32 && index < 4 {
                        index += 1;
                    }
                }
                let rem: NonNull<[u8]> =
                    NonNull::new_unchecked(remaining as *const [u8] as *mut [u8]);
                // println!("{}", index);
                alloc.lists[index].push_back(rem);

                // update allocation stats
                alloc.current_allocated_size += layout.size() as f64;
                alloc.peak_allocated_size =
                    f64::max(alloc.current_allocated_size, alloc.peak_allocated_size);
            }
            Ok(ret)
        }
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // Coalesce to a larger sized block. Always join to address 1 less than deallocated block to ensure sizing constraints
        let mut alloc: MutexGuard<'_, SegregatedFreeList> = self.lock();
        let address_to_find: usize = ptr.addr().get() + layout.size();

        let mut index: usize = 0;
        let mut node_to_coalesce: Option<NonNull<[u8]>> = None;

        while index < 5 && node_to_coalesce.is_none() {
            if !alloc.lists[index].is_empty() {
                let mut cursor: CursorMut<'_, NonNull<[u8]>> =
                    alloc.lists[index].cursor_front_mut();
                while cursor.current().is_some() {
                    // check size of space vs size needed
                    let curr = cursor.current().unwrap();
                    // println!("curr: {}", curr.addr().get());
                    if address_to_find == curr.addr().get() {
                        node_to_coalesce = cursor.remove_current();
                        break;
                    }
                    cursor.move_next();
                }
            }
            index += 1;
        }

        let mut slice: NonNull<[u8]> = NonNull::slice_from_raw_parts(ptr, layout.size());

        if node_to_coalesce.is_some() {
            // let to_append: &[u8] = &*node_to_coalesce.unwrap().as_ptr();
            // vec.extend_from_slice(to_append);
            // slice = vec.as_mut_slice();
            slice =
                NonNull::slice_from_raw_parts(ptr, layout.size() + node_to_coalesce.unwrap().len());
        }
        node_to_coalesce = Some(slice);

        // Store in corresponding list for future use
        let size: usize = node_to_coalesce.unwrap().len();
        let mut rounded_size = 1;
        index = 0;
        let mut temp: usize = size - 1;
        while temp != 0 {
            temp >>= 1;
            rounded_size <<= 1;
            if rounded_size > 32 && index < 4 {
                index += 1;
            }
        }
        alloc.lists[index].push_back(node_to_coalesce.unwrap());
        alloc.current_allocated_size -= layout.size() as f64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn test_allocate_fail() {
        let allocator: Locked<SegregatedFreeList> = Locked::new(SegregatedFreeList::new());
        let failing_layout: Layout = Layout::from_size_align(1024, 8).unwrap();

        assert_eq!(allocator.allocate(failing_layout), Err(AllocError));
    }

    #[test]
    fn test_allocate_success() {
        let allocator: Locked<SegregatedFreeList> = Locked::new(SegregatedFreeList::new());
        let layout: Layout = Layout::from_size_align(64, 8).unwrap();

        let ptr: Result<NonNull<[u8]>, AllocError> = allocator.allocate(layout);

        assert!(ptr.is_ok());
        let allocated_space: NonNull<[u8]> = ptr.unwrap();
        assert_eq!(allocated_space.len(), 64);

        let alloc: MutexGuard<'_, SegregatedFreeList> = allocator.lock();
        assert_eq!(alloc.lists[4].len(), 1);
        assert_eq!(alloc.lists[4].front().unwrap().len(), 448);
        Mutex::unlock(alloc);

        // Should use from existing list
        let layout: Layout = Layout::from_size_align(300, 8).unwrap();
        let ptr: Result<NonNull<[u8]>, AllocError> = allocator.allocate(layout);

        assert!(ptr.is_ok());
        let allocated_space: NonNull<[u8]> = ptr.unwrap();
        assert_eq!(allocated_space.len(), 300);

        let alloc: MutexGuard<'_, SegregatedFreeList> = allocator.lock();
        assert_eq!(alloc.lists[3].len(), 1);
        assert_eq!(alloc.lists[4].len(), 0);
        assert_eq!(alloc.lists[3].front().unwrap().len(), 148);
        Mutex::unlock(alloc);

        // Should allocate new node
        let layout: Layout = Layout::from_size_align(300, 8).unwrap();
        let ptr: Result<NonNull<[u8]>, AllocError> = allocator.allocate(layout);

        assert!(ptr.is_ok());
        let allocated_space: NonNull<[u8]> = ptr.unwrap();
        assert_eq!(allocated_space.len(), 300);

        let alloc: MutexGuard<'_, SegregatedFreeList> = allocator.lock();
        assert_eq!(alloc.lists[3].len(), 2);
        assert_eq!(alloc.lists[3].front().unwrap().len(), 148);
        assert_eq!(alloc.lists[3].back().unwrap().len(), 212);
    }

    #[test]
    fn test_deallocate_success() {
        let allocator: Locked<SegregatedFreeList> = Locked::new(SegregatedFreeList::new());
        let layout: Layout = Layout::from_size_align(64, 8).unwrap();

        let ptr: Result<NonNull<[u8]>, AllocError> = allocator.allocate(layout);

        assert!(ptr.is_ok());
        let allocated_space: NonNull<[u8]> = ptr.unwrap();
        // println!("{:p}", allocated_space.as_ptr());
        assert_eq!(allocated_space.len(), 64);

        let alloc: MutexGuard<'_, SegregatedFreeList> = allocator.lock();
        assert_eq!(alloc.lists[4].len(), 1);
        assert_eq!(alloc.lists[4].front().unwrap().len(), 448);
        Mutex::unlock(alloc);

        unsafe {
            let raw_first_byte: *mut u8 = allocated_space.as_mut_ptr();
            let layout: Layout = Layout::from_size_align(64, 8).unwrap();
            allocator.deallocate(NonNull::new_unchecked(raw_first_byte), layout);

            let alloc: MutexGuard<'_, SegregatedFreeList> = allocator.lock();
            // println!("{:#?}", alloc.lists);
            // println!("{}", alloc.lists[2].front().unwrap().len());
            assert_eq!(alloc.lists[4].len(), 1);
            assert_eq!(alloc.lists[4].front().unwrap().len(), 512);
            Mutex::unlock(alloc);
        }
    }

    #[test]
    fn test_allocation_stats() {
        let allocator: Locked<SegregatedFreeList> = Locked::new(SegregatedFreeList::new());
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

        let alloc: MutexGuard<'_, SegregatedFreeList> = allocator.lock();
        assert_eq!(alloc.total_size, 512 as f64);
        assert_eq!(alloc.peak_allocated_size, 384 as f64);
        assert_eq!(alloc.current_allocated_size, 288 as f64);
    }
}
