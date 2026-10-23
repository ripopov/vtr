//! Bounded payload reuse, with encoding workers returning storage in batches.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

struct Shared {
    limit: usize,
    reserved: AtomicUsize,
    pending: AtomicBool,
    returned: Mutex<Vec<Vec<u8>>>,
}

#[derive(Clone)]
pub(super) struct Return(Arc<Shared>);

pub(super) struct Pool {
    available: Vec<Vec<u8>>,
    bytes: usize,
    // Deferred checkout credits avoid an atomic update per payload while keeping admission conservative.
    pending_release: usize,
    release_threshold: usize,
    shared: Return,
}

impl Return {
    fn admit(&self, buffers: impl Iterator<Item = Vec<u8>>) -> (Vec<Vec<u8>>, usize) {
        let mut buffers: Vec<_> = buffers
            .filter(|buffer| buffer.capacity() != 0 && buffer.capacity() <= self.0.limit)
            .collect();
        let mut current = self.0.reserved.load(Ordering::Relaxed);
        loop {
            let mut room = self.0.limit.saturating_sub(current);
            let mut bytes = 0;
            buffers.retain_mut(|buffer| {
                if buffer.capacity() > room {
                    return false;
                }
                buffer.clear();
                room -= buffer.capacity();
                bytes += buffer.capacity();
                true
            });
            if bytes == 0 {
                return (buffers, 0);
            }
            match self.0.reserved.compare_exchange_weak(
                current,
                current + bytes,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return (buffers, bytes),
                Err(next) => current = next,
            }
        }
    }
    pub fn recycle(&self, buffers: impl Iterator<Item = Vec<u8>>) {
        let (buffers, bytes) = self.admit(buffers);
        if bytes != 0 {
            let mut returned = self.0.returned.lock().unwrap();
            returned.extend(buffers);
            self.0.pending.store(true, Ordering::Release);
        }
    }
}

impl Pool {
    pub fn new(limit: usize) -> Self {
        Self {
            available: Vec::new(),
            bytes: 0,
            pending_release: 0,
            release_threshold: (limit / 16).clamp(1, 64 << 10),
            shared: Return(Arc::new(Shared {
                limit,
                reserved: AtomicUsize::new(0),
                pending: AtomicBool::new(false),
                returned: Mutex::new(Vec::new()),
            })),
        }
    }
    pub fn take(&mut self, length: usize) -> Vec<u8> {
        if self.available.is_empty() && self.shared.0.pending.load(Ordering::Acquire) {
            let mut returned = self.shared.0.returned.lock().unwrap();
            self.available = std::mem::take(&mut *returned);
            self.shared.0.pending.store(false, Ordering::Relaxed);
            self.bytes = self.available.iter().map(Vec::capacity).sum();
        }
        let mut buffer = if let Some(buffer) = self.available.pop() {
            self.bytes -= buffer.capacity();
            self.pending_release += buffer.capacity();
            if self.available.is_empty() || self.pending_release >= self.release_threshold {
                self.shared
                    .0
                    .reserved
                    .fetch_sub(self.pending_release, Ordering::Relaxed);
                self.pending_release = 0;
            }
            buffer
        } else {
            Vec::new()
        };
        buffer.reserve(length);
        buffer
    }
    pub fn return_handle(&self) -> Return {
        self.shared.clone()
    }
    pub fn retained_bytes(&self) -> usize {
        self.shared.0.reserved.load(Ordering::Relaxed) - self.pending_release
    }
}
impl Drop for Pool {
    fn drop(&mut self) {
        self.shared
            .0
            .reserved
            .fetch_sub(self.bytes + self.pending_release, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deferred_checkout_credits_keep_stats_and_drop_accounting_exact() {
        let mut pool = Pool::new(1 << 20);
        let returned = pool.return_handle();
        returned.recycle((0..8).map(|_| Vec::with_capacity(8192)));
        let mut active = vec![pool.take(8192)];
        assert_eq!(pool.retained_bytes(), 7 * 8192);
        let worker = returned.clone();
        std::thread::spawn(move || worker.recycle([Vec::with_capacity(32768)].into_iter()))
            .join()
            .unwrap();
        assert_eq!(pool.retained_bytes(), 7 * 8192 + 32768);
        for _ in 0..6 {
            active.push(pool.take(8192));
        }
        assert_eq!(pool.retained_bytes(), 8192 + 32768);
        drop(pool);
        assert_eq!(returned.0.reserved.load(Ordering::Relaxed), 32768);
        assert_eq!(active.len(), 7);
    }

    #[test]
    fn worker_returns_reuse_storage_and_enforce_the_combined_budget() {
        let mut pool = Pool::new(80);
        let mut first = pool.take(40);
        first.extend_from_slice(&[3; 40]);
        let pointer = first.as_ptr();
        let returned = pool.return_handle();
        std::thread::spawn(move || returned.recycle([first].into_iter()))
            .join()
            .unwrap();
        assert_eq!(pool.retained_bytes(), 40);
        let first = pool.take(40);
        assert!(first.is_empty());
        assert_eq!(first.as_ptr(), pointer);
        assert_eq!(pool.retained_bytes(), 0);
        pool.return_handle()
            .recycle([first, Vec::with_capacity(40), Vec::with_capacity(41)].into_iter());
        assert!(pool.retained_bytes() <= 80);
        let first = pool.take(40);
        let second = pool.take(40);
        assert!(first.capacity() >= 40 && second.capacity() >= 40);
        assert_eq!(pool.retained_bytes(), 0);
        let disabled = Pool::new(0);
        disabled
            .return_handle()
            .recycle([first, second].into_iter());
        assert_eq!(disabled.retained_bytes(), 0);
    }
}
