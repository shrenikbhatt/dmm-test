use std::alloc::{GlobalAlloc, Layout, System};

use std::cell::Cell;

pub fn run_guarded<F>(f: F)
where
    F: FnOnce(),
{
    thread_local! {
        static GUARD: Cell<bool> = Cell::new(false);
    }

    GUARD.with(|guard| {
        if !guard.replace(true) {
            f();
            guard.set(false)
        }
    })
}


// #[global_allocator]
static _ALLOCATOR: MyCustomAllocator = MyCustomAllocator;
struct MyCustomAllocator;

unsafe impl GlobalAlloc for MyCustomAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        run_guarded(|| {eprintln!("bytes requested: {}\talignment: {}", &layout.size(), &layout.align());});
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

// #[global_allocator]
// static GLOBAL: MyCustomAllocator = MyCustomAllocator;