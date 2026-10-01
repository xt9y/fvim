//! Test-only allocation budgets and reproducible timing probes; no runtime cost.
use crate::buffer::Buffer;
use crate::command::Search;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::Instant;

thread_local! {
    static ALLOCATED: Cell<Option<usize>> = const { Cell::new(None) };
}

struct Allocator;

fn record(bytes: usize) {
    let _ = ALLOCATED.try_with(|counter| {
        if let Some(total) = counter.get() {
            counter.set(Some(total.saturating_add(bytes)));
        }
    });
}

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        System.alloc(layout)
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        System.alloc_zeroed(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record(new_size);
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

pub fn allocated<T>(f: impl FnOnce() -> T) -> (T, usize) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            ALLOCATED.with(|counter| counter.set(None));
        }
    }
    ALLOCATED.with(|counter| counter.set(Some(0)));
    let _reset = Reset;
    let result = f();
    (result, ALLOCATED.with(|counter| counter.get().unwrap()))
}

fn fixture() -> String {
    "alpha beta gamma delta 0123456789\n".repeat(10_000)
}

#[test]
fn opening_uses_no_temporary_normalized_file_for_lf_text() {
    let text = fixture();
    let (b, bytes) = allocated(|| Buffer::from_text(&text));
    assert!(!b.dirty());
    assert_eq!(b.lines.len(), 10_000);
    assert!(bytes < 2_200_000, "Opening allocated {bytes} bytes.");
}

#[test]
fn dirty_checks_do_not_allocate_a_file_copy() {
    let mut b = Buffer::from_text(&fixture());
    b.insert("x");
    let (dirty, bytes) = allocated(|| (0..100).all(|_| b.dirty()));
    assert!(dirty);
    assert_eq!(bytes, 0, "Dirty queries must not copy the file.");
}

#[test]
fn typing_and_undo_allocate_only_the_changed_region() {
    let mut b = Buffer::from_text(&fixture());
    b.row = 9_999;
    let (_, bytes) = allocated(|| {
        b.begin_change();
        for _ in 0..100 {
            b.insert("x");
        }
        assert!(b.end_change());
        b.undo();
        b.redo();
    });
    assert!(bytes < 65_536, "Local typing/undo allocated {bytes} bytes.");
    assert_eq!(b.lines[0], "alpha beta gamma delta 0123456789");
    assert!(b.lines[9_999].starts_with(&"x".repeat(100)));
}

#[test]
fn word_motions_do_not_copy_the_file() {
    let mut b = Buffer::from_text(&fixture());
    let (_, bytes) = allocated(|| {
        for _ in 0..100 {
            let m = crate::motion::motion(&b, 'w', 1, false).unwrap();
            b.set_offset(m.offset);
        }
    });
    assert_eq!(
        bytes, 0,
        "Word motions must visit text without file copies."
    );
    assert!(b.row > 0);
}

#[test]
fn paired_motions_and_text_objects_visit_text_without_copies() {
    let mut b = Buffer::from_text(&format!("\"hello\" (one)\n{}", fixture()));
    let (_, bytes) = allocated(|| {
        b.col = 8;
        assert_eq!(crate::motion::motion(&b, '%', 1, false).unwrap().offset, 12);
        b.col = 9;
        assert_eq!(crate::motion::object(&b, 'w', false, 1), Some((9, 12)));
        assert_eq!(crate::motion::object(&b, '(', true, 1), Some((8, 13)));
        b.col = 2;
        assert_eq!(crate::motion::object(&b, '"', true, 1), Some((0, 7)));
    });
    assert_eq!(bytes, 0, "Motions/text objects allocated {bytes} bytes.");
}

#[test]
fn searches_do_not_allocate_a_list_of_every_match() {
    let text = fixture();
    let search = Search::new("alpha".into(), true).unwrap();
    // Warm the regex engine's own cache before checking editor allocations.
    assert_eq!(search.destination(&text, 0, false, 1), Some(339_966));
    let (position, bytes) = allocated(|| search.destination(&text, 0, false, 1));
    assert_eq!(position, Some(339_966));
    assert!(
        bytes < 1024,
        "Search allocated {bytes} bytes for match positions."
    );
}

#[test]
fn grouped_edits_do_not_recopy_a_growing_undo_span() {
    let mut b = Buffer::from_text(&"alpha beta gamma delta 0123456789\n".repeat(1_000));
    let replacement = ["X".to_owned()];
    let (_, bytes) = allocated(|| {
        b.begin_change();
        for row in (0..1_000).rev() {
            b.replace_lines(row, row + 1, &replacement);
        }
        b.end_change();
        b.undo();
        b.redo();
    });
    assert!(
        bytes < 524_288,
        "Grouped row edits allocated {bytes} bytes."
    );
    assert!(b.lines.iter().all(|line| line == "X"));
}

fn probe(name: &str, f: impl FnOnce()) {
    let start = Instant::now();
    let (_, bytes) = allocated(f);
    println!(
        "PERF {name}: {:.3} ms, {bytes} allocated bytes",
        start.elapsed().as_secs_f64() * 1_000.0
    );
}

#[test]
#[ignore = "timing report; allocation budgets run in the normal suite"]
fn measured_editor_workloads() {
    let text = fixture();
    probe("open 340 KB / 10000 lines", || {
        std::hint::black_box(Buffer::from_text(&text));
    });
    let mut b = Buffer::from_text(&text);
    b.row = 9_999;
    probe("100 inserted characters", || {
        b.begin_change();
        for _ in 0..100 {
            b.insert("x");
        }
        b.end_change();
    });
    probe("100 dirty checks", || {
        for _ in 0..100 {
            std::hint::black_box(b.dirty());
        }
    });
    probe("100 undo/redo pairs", || {
        for _ in 0..100 {
            b.undo();
            b.redo();
        }
    });
    b.row = 0;
    b.col = 0;
    probe("100 word motions", || {
        for _ in 0..100 {
            let m = crate::motion::motion(&b, 'w', 1, false).unwrap();
            b.set_offset(m.offset);
        }
    });
    let search = Search::new("alpha".into(), true).unwrap();
    probe("next search / 10000 matches", || {
        std::hint::black_box(search.destination(&b.body(), 0, true, 1));
    });
    probe("backward search / 10000 matches", || {
        std::hint::black_box(search.destination(&b.body(), 0, false, 1));
    });
    let mut renderer = crate::renderer::Renderer::default();
    let mut editor = crate::editor::Editor::new(b);
    // Frame preparation is portable; native terminal IO is covered by PTY/ConPTY.
    probe("100 rendered cursor moves", || {
        for _ in 0..100 {
            editor.buffer.col ^= 1;
            renderer.prepare(&editor, (80, 24));
        }
    });
}
