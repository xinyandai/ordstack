//! SOUNDNESS.md H3(b): the destructor of an element in a frozen handle may
//! use the frame that holds the handle's block. Allocating, handing out a
//! sub-frame, `reset_above` and `trim_above` must leave the block (and its
//! neighbours) intact.

use ordstack::{Frame, SBox, SFixedPool, SFixedVec, SVec, Slot, Stack};
use std::cell::RefCell;

type Cell<'f> = RefCell<Frame<'f>>;
type Log = RefCell<Vec<u64>>;

#[derive(Clone, Copy, Debug)]
enum Mode {
    Alloc,
    Sub,
    ResetAbove,
    TrimAbove,
}

const MODES: [Mode; 4] = [Mode::Alloc, Mode::Sub, Mode::ResetAbove, Mode::TrimAbove];

fn pattern(id: u64) -> [u64; 8] {
    core::array::from_fn(|i| id.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ i as u64)
}

/// Carries a recognisable payload and, when dropped, uses the frame that
/// holds its own block, then checks that the payload survived.
struct R<'a, 'f> {
    id: u64,
    payload: [u64; 8],
    mode: Mode,
    frame: &'a Cell<'f>,
    log: &'a Log,
}

impl<'a, 'f> R<'a, 'f> {
    fn new(id: u64, mode: Mode, frame: &'a Cell<'f>, log: &'a Log) -> Self {
        R {
            id,
            payload: pattern(id),
            mode,
            frame,
            log,
        }
    }
}

impl Drop for R<'_, '_> {
    fn drop(&mut self) {
        let mut f = self.frame.borrow_mut();
        let end = f.end_offset();
        match self.mode {
            Mode::Alloc => {}
            Mode::Sub => {
                let mut s = f.sub();
                let tmp = s.alloc_slice_copy(&[0xEEu8; 300]);
                assert!(tmp.iter().all(|&b| b == 0xEE));
            }
            Mode::ResetAbove => f.reset_above(),
            Mode::TrimAbove => f.trim_above(),
        }
        assert_eq!(f.end_offset(), end, "reclaiming above must not move end");
        // Write over everything the frame hands out next.
        let fill = f.alloc_slice_copy(&[u64::MAX; 64]);
        let one = f.alloc(0xDEAD_BEEF_u64);
        assert!(f.end_offset() >= end + 512 + 8);
        assert!(fill.iter().all(|&w| w == u64::MAX));
        assert_eq!(*one, 0xDEAD_BEEF);
        assert_eq!(self.payload, pattern(self.id), "own block was overwritten");
        self.log.borrow_mut().push(self.id);
    }
}

/// A box allocated before and one after the container: both must be intact.
fn check_neighbours(before: &SBox<'_, [u64; 8]>, after: &SBox<'_, [u64; 8]>) {
    assert_eq!(**before, pattern(1000));
    assert_eq!(**after, pattern(2000));
}

#[test]
fn sbox_destructor_allocates_from_its_own_frame() {
    // The program from the review, plus a payload check.
    let mut stack = Stack::with_reserve(1 << 16);
    let frame = RefCell::new(stack.frame());
    let log = Log::default();
    for mode in MODES {
        let before = frame.borrow_mut().alloc(pattern(1000));
        let value = frame.borrow_mut().alloc(R::new(7, mode, &frame, &log));
        let after = frame.borrow_mut().alloc(pattern(2000));
        drop(value);
        check_neighbours(&before, &after);
    }
    assert_eq!(*log.borrow(), [7, 7, 7, 7]);
}

#[test]
fn sbox_reentrant_alloc_of_a_plain_value() {
    struct Reenter<'a, 'f>(&'a Cell<'f>);
    impl Drop for Reenter<'_, '_> {
        fn drop(&mut self) {
            let value = self.0.borrow_mut().alloc(42u64);
            assert_eq!(*value, 42);
        }
    }
    let mut stack = Stack::with_reserve(4096);
    let frame = RefCell::new(stack.frame());
    let value = frame.borrow_mut().alloc(Reenter(&frame));
    drop(value);
}

#[test]
fn sslice_destructors_use_their_frame() {
    for mode in MODES {
        let mut stack = Stack::with_reserve(1 << 16);
        let frame = RefCell::new(stack.frame());
        let log = Log::default();
        let before = frame.borrow_mut().alloc(pattern(1000));
        // Frozen through `SVec::freeze`, which raises `end` past the elements.
        let slice = {
            let mut g = frame.borrow_mut();
            let mut v = SVec::new_in(&mut g);
            for id in 0..4 {
                v.push(R::new(id, mode, &frame, &log));
            }
            v.freeze()
        };
        let after = frame.borrow_mut().alloc(pattern(2000));
        assert_eq!(slice.len(), 4);
        drop(slice);
        check_neighbours(&before, &after);
        assert_eq!(*log.borrow(), [3, 2, 1, 0], "{mode:?}");
    }
}

#[test]
fn sfixedvec_destructors_use_their_frame() {
    for mode in MODES {
        let mut stack = Stack::with_reserve(1 << 16);
        let frame = RefCell::new(stack.frame());
        let log = Log::default();
        let before = frame.borrow_mut().alloc(pattern(1000));
        let mut v = SFixedVec::with_capacity_in(6, &mut frame.borrow_mut());
        for id in 0..4 {
            v.push(R::new(id, mode, &frame, &log));
            // The frame keeps allocating while the vector is filled.
            let _ = frame.borrow_mut().alloc(id);
        }
        let after = frame.borrow_mut().alloc(pattern(2000));
        // Spare capacity must survive the destructors' allocations too.
        v.push(R::new(4, mode, &frame, &log));
        assert_eq!(v.len(), 5);
        drop(v);
        check_neighbours(&before, &after);
        assert_eq!(*log.borrow(), [4, 3, 2, 1, 0], "{mode:?}");
    }
}

#[test]
fn sfixedvec_into_slice_destructors_use_their_frame() {
    for mode in MODES {
        let mut stack = Stack::with_reserve(1 << 16);
        let frame = RefCell::new(stack.frame());
        let log = Log::default();
        let mut v = SFixedVec::with_capacity_in(8, &mut frame.borrow_mut());
        for id in 0..3 {
            v.push(R::new(id, mode, &frame, &log));
        }
        let s = v.into_slice();
        let after = frame.borrow_mut().alloc(pattern(2000));
        drop(s);
        assert_eq!(*after, pattern(2000));
        assert_eq!(*log.borrow(), [2, 1, 0], "{mode:?}");
    }
}

#[test]
fn sfixedpool_destructor_uses_its_frame() {
    for mode in MODES {
        let mut stack = Stack::with_reserve(1 << 16);
        let frame = RefCell::new(stack.frame());
        let log = Log::default();
        let before = frame.borrow_mut().alloc(pattern(1000));
        let mut pool = SFixedPool::<Slot<R<'_, '_>>>::with_capacity_in(2, &mut frame.borrow_mut());
        let first = pool.alloc(R::new(1, mode, &frame, &log)).unwrap();
        let after = frame.borrow_mut().alloc(pattern(2000));

        pool.free(first);
        assert_eq!(*after, pattern(2000));
        check_neighbours(&before, &after);
        assert_eq!(*log.borrow(), [1], "{mode:?}");
    }
}

#[test]
fn partly_consumed_into_iter_destructors_use_their_frame() {
    for mode in MODES {
        let mut stack = Stack::with_reserve(1 << 16);
        let frame = RefCell::new(stack.frame());
        let log = Log::default();
        let before = frame.borrow_mut().alloc(pattern(1000));
        let mut v = SFixedVec::with_capacity_in(4, &mut frame.borrow_mut());
        for id in 0..4 {
            v.push(R::new(id, mode, &frame, &log));
        }
        let mut it = v.into_slice().into_iter();
        let after = frame.borrow_mut().alloc(pattern(2000));
        // Each yielded element is dropped at once, re-entering the frame
        // while the rest of the block is still owned by the iterator.
        drop(it.next().unwrap());
        drop(it.next_back().unwrap());
        assert_eq!(*log.borrow(), [0, 3]);
        assert_eq!(it.len(), 2);
        // Dropping the iterator drops the remaining two, back to front.
        drop(it);
        check_neighbours(&before, &after);
        assert_eq!(*log.borrow(), [0, 3, 2, 1], "{mode:?}");
    }
}

#[test]
fn destructor_in_a_sub_frame_uses_that_sub_frame() {
    let mut stack = Stack::with_reserve(1 << 16);
    let mut top = stack.frame();
    let kept = top.alloc(pattern(1000));
    let sub = RefCell::new(top.sub());
    let log = Log::default();
    for mode in MODES {
        let value = sub.borrow_mut().alloc(R::new(9, mode, &sub, &log));
        drop(value);
    }
    assert_eq!(*kept, pattern(1000));
    assert_eq!(*log.borrow(), [9, 9, 9, 9]);
}
