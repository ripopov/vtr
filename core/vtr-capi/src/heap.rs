//! libvtr's heap: its own, not the C library's. Heap corruption in the
//! program under test can leave glibc's arena locked for ever (a double free
//! detected under the arena lock aborts with the lock held), and a crash
//! guard must still be able to finish the trace. Small blocks come from
//! mimalloc, without its `override` feature, so the application's `malloc`
//! is untouched. Blocks of 1 MiB and more are mapped directly and grow in
//! place with `mremap`, as glibc grows them: mimalloc's copy-on-grow held
//! both copies of the largest buffers and raised the peak memory of a
//! 60-million-change run from 343 to 600 MiB.

use std::alloc::{GlobalAlloc, Layout};

pub struct VtrHeap;

#[cfg(target_os = "linux")]
const LARGE: usize = 1 << 20;
#[cfg(not(target_os = "linux"))]
const LARGE: usize = usize::MAX;
const PAGE: usize = 4096;

fn large(layout: &Layout) -> bool {
    layout.size() >= LARGE && layout.align() <= PAGE
}

#[cfg(target_os = "linux")]
unsafe fn map(size: usize) -> *mut u8 {
    let p = libc::mmap(std::ptr::null_mut(), size, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0);
    if p == libc::MAP_FAILED {
        std::ptr::null_mut()
    } else {
        p.cast()
    }
}

#[cfg(not(target_os = "linux"))]
unsafe fn map(_: usize) -> *mut u8 {
    unreachable!()
}

unsafe impl GlobalAlloc for VtrHeap {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if large(&layout) {
            return map(layout.size());
        }
        mimalloc::MiMalloc.alloc(layout)
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if large(&layout) {
            return map(layout.size()); // anonymous mappings are zeroed
        }
        mimalloc::MiMalloc.alloc_zeroed(layout)
    }

    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        if large(&layout) {
            #[cfg(target_os = "linux")]
            libc::munmap(p.cast(), layout.size());
            return;
        }
        mimalloc::MiMalloc.dealloc(p, layout)
    }

    #[inline]
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new = Layout::from_size_align_unchecked(new_size, layout.align());
        match (large(&layout), large(&new)) {
            (false, false) => mimalloc::MiMalloc.realloc(p, layout, new_size),
            #[cfg(target_os = "linux")]
            (true, true) => {
                let q = libc::mremap(p.cast(), layout.size(), new_size, libc::MREMAP_MAYMOVE);
                if q == libc::MAP_FAILED {
                    std::ptr::null_mut()
                } else {
                    q.cast()
                }
            }
            _ => {
                let q = self.alloc(new);
                if !q.is_null() {
                    std::ptr::copy_nonoverlapping(p, q, layout.size().min(new_size));
                    self.dealloc(p, layout);
                }
                q
            }
        }
    }
}
