use std::sync::{Mutex, MutexGuard};

pub trait Lock<A> {
    fn lock(&self) -> MutexGuard<A>;
}

pub struct Locked<A> {
    inner: Mutex<A>,
}

impl<A> Locked<A> {
    pub const fn new(inner: A) -> Self {
        Locked {
            inner: Mutex::new(inner),
        }
    }
}

impl<A> Lock<A> for Locked<A> {
    fn lock(&self) -> MutexGuard<A> {
        self.inner.lock().unwrap()
    }
}
