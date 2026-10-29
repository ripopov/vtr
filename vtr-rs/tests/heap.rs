//! VTR's memory comes from the Rust global allocator, zstd's included: a program that gives
//! VTR a heap of its own keeps every VTR allocation off the C library's heap, which a crash
//! can leave locked.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use vtr::codec::{Compression, Compressor};

struct Counting;

static BYTES: AtomicUsize = AtomicUsize::new(0);

// SAFETY: Allocation and deallocation preserve System layouts and pointers; counters are atomic or thread-local.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The allocator caller supplies the layout and pointer required by System.
        unsafe {
            BYTES.fetch_add(layout.size(), Ordering::Relaxed);
            System.alloc(layout)
        }
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        // SAFETY: The allocator caller supplies the layout and pointer required by System.
        unsafe {
            System.dealloc(p, layout);
        }
    }
}

#[global_allocator]
static HEAP: Counting = Counting;

#[test]
fn zstd_contexts_allocate_from_the_global_allocator() {
    let input: Vec<u8> = (0..1u32 << 20)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8 & 0x0f)
        .collect();
    let mut out = Vec::with_capacity(2 << 20);
    let mut c = Compressor::new();
    let before = BYTES.load(Ordering::Relaxed);
    c.compress_into(Compression::ZSTD_DEFAULT, &input, &mut out)
        .unwrap();
    let zstd = BYTES.load(Ordering::Relaxed) - before;
    assert!(
        zstd > 512 << 10,
        "zstd allocated only {zstd} bytes through the global allocator"
    );
    assert!(out.len() < input.len() / 2);
    let mut d = vtr::codec::Decompressor::new();
    assert_eq!(d.decompress(&out).unwrap(), input);
}
