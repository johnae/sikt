//! Matching doesn't allocate per match: the scratch space is reused and a
//! match keeps a few captures inline.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Allocations while finding every match in `text`.
fn allocations(pattern: &sikt::Pattern, text: &str) -> usize {
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    let mut found = 0;
    for m in pattern.find_iter(text) {
        let m = m.unwrap();
        found += m.get("key").map_or(0, str::len) + m.get("value").map_or(0, str::len);
    }
    assert!(found > 0);
    ALLOCATIONS.load(Ordering::Relaxed) - before
}

#[test]
fn matches_with_captures_dont_allocate() {
    let pattern = sikt::Pattern::builder()
        .line_mode(true)
        .build(r#"key: 1+ letter "=" value: (until (space or text end))"#)
        .unwrap();
    let few = allocations(&pattern, &"a=1 bb=22\n".repeat(10));
    let many = allocations(&pattern, &"a=1 bb=22\n".repeat(1_000));
    assert_eq!(few, many, "allocations grew with the number of matches");
}
