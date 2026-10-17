//! libvtr's heap: its own, not the C library's.

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
    // SAFETY: Anonymous mmap needs no input memory; failed mappings are converted to null.
    unsafe {
        let p = libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        );
        if p == libc::MAP_FAILED {
            std::ptr::null_mut()
        } else {
            p.cast()
        }
    }
}

#[cfg(not(target_os = "linux"))]
unsafe fn map(_: usize) -> *mut u8 {
    unreachable!()
}

// SAFETY: Layouts deterministically select mmap or MiMalloc, preserving alignment and matching frees.
unsafe impl GlobalAlloc for VtrHeap {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The caller supplies a valid layout; mmap satisfies page alignment and MiMalloc handles smaller blocks.
        unsafe {
            if large(&layout) {
                return map(layout.size());
            }
            mimalloc::MiMalloc.alloc(layout)
        }
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: Anonymous mappings start zeroed; other valid layouts are delegated to MiMalloc.
        unsafe {
            if large(&layout) {
                return map(layout.size());
            }
            mimalloc::MiMalloc.alloc_zeroed(layout)
        }
    }

    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        // SAFETY: The original layout selects the allocator that owns p; the caller relinquishes the live allocation.
        unsafe {
            if large(&layout) {
                #[cfg(target_os = "linux")]
                libc::munmap(p.cast(), layout.size());
                return;
            }
            mimalloc::MiMalloc.dealloc(p, layout);
        }
    }

    #[inline]
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: The caller supplies a live allocation and valid new size; copying is bounded by both allocations.
        unsafe {
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
}
