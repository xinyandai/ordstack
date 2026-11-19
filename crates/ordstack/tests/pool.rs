use ordstack::{Frame, PoolSlot, SFixedPool, SPool, Slot, Stack};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::thread_local;

#[test]
fn zero_capacity_fixed_pool_is_always_full() {
    let mut stack = Stack::with_reserve(0);
    let mut frame = stack.frame();
    let mut pool = SFixedPool::<Slot<u32>>::with_capacity_in(0, &mut frame);
    assert_eq!(pool.capacity(), 0);
    assert_eq!(pool.alloc(7), None);
    assert!(pool.is_empty());
}

#[test]
fn slot_numbers_keep_the_full_usize_width() {
    let number = usize::MAX - 1;
    let empty = Slot::<u32>::empty(Some(number));
    assert_eq!(empty.next(), Some(number));
    let (full, index) = empty.occupy(number, 7);
    assert_eq!(index.slot(), number);
    assert_eq!(Slot::<u32>::slot(index), number);
    assert_eq!(full.vacate(Some(number)).next(), Some(number));
}

#[test]
#[should_panic(expected = "occupied pool slot has no free-list link")]
fn occupied_slot_has_no_free_list_link() {
    let empty = Slot::<u32>::empty(None);
    let (occupied, _) = PoolSlot::occupy(&empty, 0, 7);
    PoolSlot::next(&occupied);
}

#[test]
#[should_panic(expected = "cannot occupy a full pool slot")]
fn full_slot_cannot_be_occupied_again() {
    let empty = Slot::<u32>::empty(None);
    let (occupied, _) = PoolSlot::occupy(&empty, 0, 7);
    let _ = PoolSlot::occupy(&occupied, 0, 8);
}

#[test]
#[should_panic(expected = "cannot release an empty pool slot")]
fn empty_slot_cannot_be_released() {
    PoolSlot::vacate(&Slot::<u32>::empty(None), None);
}

#[test]
fn fixed_pool_reuses_slots_and_exhaustion_does_not_leak_frame() {
    let mut stack = Stack::new();
    let mut frame = stack.frame();
    let mut pool = SFixedPool::<Slot<u32>>::with_capacity_in(2, &mut frame);
    let first = pool.alloc(10).unwrap();
    let second = pool.alloc(20).unwrap();

    assert_eq!(pool.len(), 2);
    assert!(pool.alloc(30).is_none());
    assert_eq!(pool.get(first), Some(&10));
    assert_eq!(pool.get(second), Some(&20));

    pool.free(second);
    assert_eq!((pool.len(), pool.is_empty()), (1, false));
    let reused_second = pool.alloc(30).unwrap();
    assert_eq!(reused_second, second);
    let number = frame.alloc(99u32);
    assert_eq!(
        (*number, pool.get(first), pool.get(reused_second)),
        (99, Some(&10), Some(&30))
    );
}

struct CustomSlot {
    next: Option<usize>,
    value: u32,
    occupied: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CustomIndex(usize);

impl PoolSlot for CustomSlot {
    type Value = u32;
    type Index = CustomIndex;

    fn empty(next: Option<usize>) -> Self {
        Self {
            next,
            value: 0,
            occupied: false,
        }
    }

    fn occupy(&self, slot: usize, value: u32) -> (Self, CustomIndex) {
        assert!(!self.occupied);
        (
            Self {
                next: None,
                value,
                occupied: true,
            },
            CustomIndex(slot),
        )
    }

    fn vacate(&self, next: Option<usize>) -> Self {
        assert!(self.occupied);
        Self {
            next,
            value: 0,
            occupied: false,
        }
    }

    fn next(&self) -> Option<usize> {
        assert!(!self.occupied);
        self.next
    }

    fn slot(index: CustomIndex) -> usize {
        index.0
    }

    fn get(&self, _index: CustomIndex) -> Option<&u32> {
        self.occupied.then_some(&self.value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GenerationalIndex {
    slot: usize,
    generation: u64,
}

enum GenerationalSlot<T> {
    Empty {
        next: Option<usize>,
        generation: u64,
    },
    Full {
        value: T,
        generation: u64,
    },
}

impl<T> PoolSlot for GenerationalSlot<T> {
    type Value = T;
    type Index = GenerationalIndex;

    fn empty(next: Option<usize>) -> Self {
        Self::Empty {
            next,
            generation: 0,
        }
    }

    fn occupy(&self, slot: usize, value: T) -> (Self, GenerationalIndex) {
        let generation = match self {
            Self::Empty { generation, .. } => {
                generation.checked_add(1).expect("generation exhausted")
            }
            Self::Full { .. } => panic!("cannot occupy a full slot"),
        };
        (
            Self::Full { value, generation },
            GenerationalIndex { slot, generation },
        )
    }

    fn vacate(&self, next: Option<usize>) -> Self {
        let generation = match self {
            Self::Empty { .. } => panic!("cannot release an empty slot"),
            Self::Full { generation, .. } => *generation,
        };
        Self::Empty { next, generation }
    }

    fn next(&self) -> Option<usize> {
        match self {
            Self::Empty { next, .. } => *next,
            Self::Full { .. } => panic!("full slot has no free-list link"),
        }
    }

    fn slot(index: GenerationalIndex) -> usize {
        index.slot
    }

    fn get(&self, index: GenerationalIndex) -> Option<&T> {
        match self {
            Self::Empty { .. } => None,
            Self::Full { value, generation } if *generation == index.generation => Some(value),
            Self::Full { .. } => None,
        }
    }
}

#[test]
fn generational_slot_indices_detect_use_after_free() {
    let mut stack = Stack::new();
    let mut frame = stack.frame();
    let mut pool = SFixedPool::<GenerationalSlot<&str>>::with_capacity_in(1, &mut frame);

    let old = pool.alloc("old").unwrap();
    pool.free(old);
    let current = pool.alloc("current").unwrap();

    assert_eq!(current.slot, old.slot);
    assert_ne!(current.generation, old.generation);
    assert_eq!(pool.get(old), None);
    assert_eq!(pool.get(current), Some(&"current"));
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pool.free(old))).is_err());
    assert_eq!(pool.get(current), Some(&"current"));
}

#[test]
fn generations_survive_freeze_and_freeze_into() {
    let mut stack = Stack::new();
    let mut output_stack = Stack::new();
    let mut frame = stack.frame();
    let mut pool = SPool::<GenerationalSlot<u32>>::with_capacity_in(1, &mut frame);
    let stale = pool.alloc(10);
    pool.free(stale);

    let mut frozen = pool.freeze();
    let current = frozen.alloc(20).unwrap();
    assert_eq!(frozen.get(stale), None);
    assert_eq!(frozen.get(current), Some(&20));

    let mut pool = SPool::<GenerationalSlot<u32>>::with_capacity_in(1, &mut frame);
    let stale_moved = pool.alloc(11);
    pool.free(stale_moved);
    let mut output = output_stack.frame();
    let mut moved = pool.freeze_into(&mut output);
    let moved_current = moved.alloc(30).unwrap();
    assert_eq!(moved_current.slot, stale_moved.slot);
    assert_ne!(moved_current.generation, stale_moved.generation);
    assert_eq!(moved.get(stale_moved), None);
    assert_eq!(moved.get(moved_current), Some(&30));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    None,
    Empty,
    Occupy,
    Vacate,
    Next,
    Slot,
    Get,
}

thread_local! {
    static FAULT: Cell<Fault> = const { Cell::new(Fault::None) };
}

fn fault(expected: Fault) {
    FAULT.with(|fault| assert_ne!(fault.get(), expected, "injected pool slot failure"));
}

struct FaultSlot {
    next: Option<Option<usize>>,
    value: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FaultIndex(usize);

impl PoolSlot for FaultSlot {
    type Value = u32;
    type Index = FaultIndex;

    fn empty(next: Option<usize>) -> Self {
        fault(Fault::Empty);
        Self {
            next: Some(next),
            value: 0,
        }
    }

    fn occupy(&self, slot: usize, value: u32) -> (Self, FaultIndex) {
        fault(Fault::Occupy);
        assert!(self.next.is_some());
        (Self { next: None, value }, FaultIndex(slot))
    }

    fn vacate(&self, next: Option<usize>) -> Self {
        fault(Fault::Vacate);
        assert!(self.next.is_none());
        Self {
            next: Some(next),
            value: 0,
        }
    }

    fn next(&self) -> Option<usize> {
        fault(Fault::Next);
        self.next.expect("occupied slot has no free-list link")
    }

    fn slot(index: FaultIndex) -> usize {
        fault(Fault::Slot);
        index.0
    }

    fn get(&self, _index: FaultIndex) -> Option<&u32> {
        fault(Fault::Get);
        self.next.is_none().then_some(&self.value)
    }
}

fn set_fault(fault: Fault) {
    FAULT.with(|cell| cell.set(fault));
}

#[test]
fn pool_slot_hook_panics_leave_the_pool_usable() {
    set_fault(Fault::Empty);
    let mut stack = Stack::new();
    let mut frame = stack.frame();
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = SFixedPool::<FaultSlot>::with_capacity_in(1, &mut frame);
        }))
        .is_err()
    );
    let _kept = frame.alloc(7_u32);

    set_fault(Fault::None);
    let mut pool = SFixedPool::<FaultSlot>::with_capacity_in(1, &mut frame);
    let attempts = [Fault::Next, Fault::Occupy];

    for hook in attempts {
        set_fault(hook);
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pool.alloc(10))).is_err());
        set_fault(Fault::None);
        assert_eq!((pool.len(), pool.is_empty()), (0, true));
    }

    let index = pool.alloc(10).unwrap();
    set_fault(Fault::Slot);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pool.get(index))).is_err());
    set_fault(Fault::None);
    assert_eq!(pool.get(index), Some(&10));

    set_fault(Fault::Get);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pool.get(index))).is_err());
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pool.free(index))).is_err());
    set_fault(Fault::None);
    assert_eq!(pool.get(index), Some(&10));

    set_fault(Fault::Vacate);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pool.free(index))).is_err());
    set_fault(Fault::None);
    assert_eq!(pool.get(index), Some(&10));
    pool.free(index);
    assert!(pool.is_empty());
}

#[test]
fn full_fixed_pool_drops_rejected_value() {
    struct Dropped(Rc<Cell<usize>>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    let dropped = Rc::new(Cell::new(0));
    let mut stack = Stack::new();
    let mut frame = stack.frame();
    let mut pool = SFixedPool::<Slot<Dropped>>::with_capacity_in(1, &mut frame);
    let _index = pool.alloc(Dropped(dropped.clone())).unwrap();
    assert!(pool.alloc(Dropped(dropped.clone())).is_none());
    assert_eq!(dropped.get(), 1);
}

#[test]
fn failed_growth_leaves_pool_values_and_counts_intact() {
    struct Value(Rc<Cell<usize>>);
    impl Drop for Value {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    let dropped = Rc::new(Cell::new(0));
    let mut stack = Stack::with_reserve(4096);
    let mut frame = stack.frame();
    let mut pool = SPool::<Slot<Value>>::with_capacity_in(1, &mut frame);
    let first = pool.alloc(Value(dropped.clone()));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        loop {
            pool.alloc(Value(dropped.clone()));
        }
    }));
    assert!(result.is_err(), "the pool unexpectedly fit in the frame");

    assert_eq!(dropped.get(), 1);
    assert!(!pool.is_empty());
    assert!(pool.get(first).is_some());
    let len = pool.len();
    drop(pool);
    assert_eq!(dropped.get(), len + 1);
}

#[test]
fn fixed_pool_survives_frame_reclamation() {
    let mut stack = Stack::with_reserve(1 << 16);
    let mut frame = stack.frame();
    let mut pool = SFixedPool::<Slot<u32>>::with_capacity_in(1, &mut frame);
    let index = pool.alloc(41).unwrap();
    {
        let mut sub = frame.sub();
        let _temporary = sub.alloc([0_u8; 1024]);
    }
    frame.reset_above();
    frame.trim_above();

    assert_eq!(pool.get(index), Some(&41));
    let after = frame.alloc(42_u32);
    assert_eq!((*after, pool.get(index)), (42, Some(&41)));
}

#[test]
fn a_panicking_value_drop_finishes_the_free_transition() {
    struct DropState {
        drops: Cell<usize>,
        armed: Cell<bool>,
    }
    struct Value(Rc<DropState>);
    impl Drop for Value {
        fn drop(&mut self) {
            self.0.drops.set(self.0.drops.get() + 1);
            if !std::thread::panicking() && self.0.armed.replace(false) {
                panic!("drop failed");
            }
        }
    }

    let state = Rc::new(DropState {
        drops: Cell::new(0),
        armed: Cell::new(true),
    });
    let mut stack = Stack::new();
    let mut frame = stack.frame();
    let mut pool = SFixedPool::<Slot<Value>>::with_capacity_in(1, &mut frame);
    let index = pool.alloc(Value(state.clone())).unwrap();

    let free_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pool.free(index)));
    state.armed.set(false);
    assert!(free_result.is_err());
    assert_eq!(
        (pool.len(), pool.is_empty(), pool.get(index).is_none()),
        (0, true, true)
    );
    assert_eq!(state.drops.get(), 1);

    let replacement = pool.alloc(Value(state.clone())).unwrap();
    assert!(pool.get(replacement).is_some());
}

fn check_slot_reuse<T: PoolSlot<Value = u32>>() {
    let mut stack = Stack::new();
    let mut frame = stack.frame();
    let mut pool = SFixedPool::<T>::with_capacity_in(2, &mut frame);

    let first = pool.alloc(10).unwrap();
    let second = pool.alloc(20).unwrap();
    assert!(pool.alloc(30).is_none());
    pool.free(first);
    pool.free(second);
    assert_eq!(pool.get(first), None);
    assert_eq!(pool.get(second), None);

    let reused_second = pool.alloc(30).unwrap();
    let reused_first = pool.alloc(40).unwrap();
    assert_eq!(T::slot(reused_second), T::slot(second));
    assert_eq!(T::slot(reused_first), T::slot(first));
    assert_eq!(pool.get(reused_second), Some(&30));
    assert_eq!(pool.get(reused_first), Some(&40));
}

#[test]
fn default_and_custom_slot_encodings_reuse_freed_slots() {
    check_slot_reuse::<Slot<u32>>();
    check_slot_reuse::<CustomSlot>();
}

#[test]
fn zero_capacity_dynamic_pool_grows_on_first_alloc() {
    let mut stack = Stack::with_reserve(4096);
    let mut frame = stack.frame();
    let mut pool = SPool::<Slot<u32>>::with_capacity_in(0, &mut frame);
    assert_eq!(pool.capacity(), 0);
    let index = pool.alloc(7);
    assert_eq!(pool.get(index), Some(&7));
}

#[test]
#[should_panic(expected = "capacity overflow")]
fn dynamic_pool_rejects_capacity_overflow() {
    let mut stack = Stack::with_reserve(0);
    let mut frame = stack.frame();
    let _pool = SPool::<Slot<u32>>::with_capacity_in(usize::MAX, &mut frame);
}

fn check_dynamic_pool<T: PoolSlot<Value = u32>>() {
    let mut stack = Stack::new();
    let mut frame = stack.frame();
    let mut pool = SPool::<T>::with_capacity_in(2, &mut frame);
    let first = pool.alloc(10);
    let second = pool.alloc(20);
    let third = pool.alloc(30);
    assert!(pool.capacity() >= 3);
    assert_eq!(pool.len(), 3);
    assert_eq!(pool.get(first), Some(&10));
    assert_eq!(pool.get(third), Some(&30));

    let capacity = pool.capacity();
    pool.free(second);
    assert_eq!(pool.get(second), None);
    let reused = pool.alloc(40);
    assert_eq!(T::slot(reused), T::slot(second));
    assert_eq!(pool.capacity(), capacity);
    pool.free(reused);

    let mut frozen = pool.freeze();
    assert_eq!(frozen.capacity(), capacity);
    let number = frame.alloc(99u32);
    assert_eq!(*number, 99);
    assert_eq!(frozen.get(first), Some(&10));
    assert_eq!(frozen.get(third), Some(&30));
    assert_eq!(frozen.get(second), None);
    for value in 0..capacity - 2 {
        let index = frozen.alloc(value as u32).unwrap();
        assert_eq!(frozen.get(index), Some(&(value as u32)));
    }
    assert!(frozen.alloc(50).is_none());
    frozen.free(second);
    let index = frozen.alloc(60).unwrap();
    assert_eq!(T::slot(index), T::slot(second));
    assert_eq!(frozen.get(index), Some(&60));
    assert_eq!(*number, 99);
}

#[test]
fn dynamic_pool_grows_reuses_and_freezes() {
    check_dynamic_pool::<Slot<u32>>();
    check_dynamic_pool::<CustomSlot>();
}

#[test]
fn dynamic_pool_reports_empty_and_nonempty_states() {
    let mut stack = Stack::new();
    let mut frame = stack.frame();
    let mut pool = SPool::<Slot<u32>>::with_capacity_in(1, &mut frame);

    assert!(pool.is_empty());
    let index = pool.alloc(1);
    assert!(!pool.is_empty());
    pool.free(index);
    assert!(pool.is_empty());
}

#[test]
fn freezing_an_unused_pool_preserves_reserved_capacity() {
    let mut stack = Stack::new();
    let mut frame = stack.frame();
    let pool = SPool::<Slot<u32>>::with_capacity_in(5, &mut frame);
    let capacity = pool.capacity();
    let mut frozen = pool.freeze();
    let number = frame.alloc(99u32);

    assert_eq!(frozen.capacity(), capacity);
    for value in 0..capacity {
        assert!(frozen.alloc(value as u32).is_some());
    }
    assert!(frozen.alloc(50).is_none());
    assert_eq!(*number, 99);
}

#[test]
fn freezing_transfers_value_ownership_without_dropping_live_values() {
    struct Value(Rc<Cell<u32>>);

    impl Drop for Value {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    let drops = Rc::new(Cell::new(0));
    let mut stack = Stack::new();
    let mut frame = stack.frame();
    let mut pool = SPool::<Slot<Value>>::with_capacity_in(1, &mut frame);
    let first = pool.alloc(Value(drops.clone()));
    let _second = pool.alloc(Value(drops.clone()));
    pool.free(first);
    assert_eq!(drops.get(), 1);
    let frozen = pool.freeze();
    assert_eq!(drops.get(), 1);
    drop(frozen);
    assert_eq!(drops.get(), 2);
}

#[test]
fn pool_can_descend_and_freeze_into_another_stack() {
    fn add_above(pool: &mut SPool<'_, '_, Slot<u32>>) {
        let mut above = pool.above();
        assert_eq!(*above.alloc(7), 7);
    }

    let mut results = Stack::new();
    let mut scratch = Stack::new();
    let (mut output, mut work) = (results.frame(), scratch.frame());
    let mut pool = SPool::<Slot<u32>>::with_capacity_in(2, &mut work);
    let first = pool.alloc(10);
    add_above(&mut pool);
    pool.alloc(20);
    let frozen = pool.freeze_into(&mut output);
    assert_eq!(frozen.get(first), Some(&10));
    let value = output.alloc(30);
    assert_eq!((*value, frozen.len()), (30, 2));
}

struct FrameValue<'a, 'f> {
    number: u32,
    frame: &'a RefCell<Frame<'f>>,
}

#[derive(Clone, Copy)]
struct FrameIndex<'a, 'f> {
    slot: usize,
    frame: &'a RefCell<Frame<'f>>,
}

enum FrameSlot<'a, 'f> {
    Empty {
        next: Option<usize>,
        frame: Option<&'a RefCell<Frame<'f>>>,
    },
    Full {
        value: FrameValue<'a, 'f>,
    },
}

impl<'a, 'f> FrameSlot<'a, 'f> {
    fn allocate(frame: &RefCell<Frame<'f>>) {
        assert_eq!(*frame.borrow_mut().alloc(0xFEED_u32), 0xFEED);
    }
}

impl<'a, 'f> PoolSlot for FrameSlot<'a, 'f> {
    type Value = FrameValue<'a, 'f>;
    type Index = FrameIndex<'a, 'f>;

    fn empty(next: Option<usize>) -> Self {
        Self::Empty { next, frame: None }
    }

    fn occupy(&self, slot: usize, value: FrameValue<'a, 'f>) -> (Self, FrameIndex<'a, 'f>) {
        Self::allocate(value.frame);
        let frame = value.frame;
        (Self::Full { value }, FrameIndex { slot, frame })
    }

    fn vacate(&self, next: Option<usize>) -> Self {
        let Self::Full { value } = self else {
            panic!("cannot vacate an empty slot");
        };
        Self::allocate(value.frame);
        Self::Empty {
            next,
            frame: Some(value.frame),
        }
    }

    fn next(&self) -> Option<usize> {
        let Self::Empty { next, frame } = self else {
            panic!("occupied slot has no free-list link");
        };
        if let Some(frame) = frame {
            Self::allocate(frame);
        }
        *next
    }

    fn slot(index: FrameIndex<'a, 'f>) -> usize {
        Self::allocate(index.frame);
        index.slot
    }

    fn get(&self, index: FrameIndex<'a, 'f>) -> Option<&FrameValue<'a, 'f>> {
        let Self::Full { value } = self else {
            return None;
        };
        Self::allocate(index.frame);
        Some(value)
    }
}

#[test]
fn fixed_pool_slot_hooks_can_allocate_from_the_same_frame() {
    let mut stack = Stack::with_reserve(1 << 16);
    let frame = RefCell::new(stack.frame());
    let before = frame.borrow_mut().alloc(0x1234_u32);
    let mut pool = SFixedPool::<FrameSlot<'_, '_>>::with_capacity_in(1, &mut frame.borrow_mut());
    let index = pool.alloc(FrameValue {
        number: 41,
        frame: &frame,
    });
    let index = index.unwrap();
    assert_eq!(pool.get(index).unwrap().number, 41);
    pool.free(index);
    let index = pool
        .alloc(FrameValue {
            number: 42,
            frame: &frame,
        })
        .unwrap();
    assert_eq!(pool.get(index).unwrap().number, 42);
    assert_eq!(*before, 0x1234);
}

#[test]
#[should_panic(expected = "pool index does not refer to an occupied slot")]
fn freeing_the_same_slot_twice_panics() {
    let mut stack = Stack::new();
    let mut frame = stack.frame();
    let mut pool = SFixedPool::<Slot<u32>>::with_capacity_in(1, &mut frame);
    let index = pool.alloc(1).unwrap();
    pool.free(index);
    pool.free(index);
}

#[test]
fn an_index_from_another_pool_misses_get() {
    let mut stack = Stack::new();
    let mut frame = stack.frame();
    let mut small = SFixedPool::<Slot<u32>>::with_capacity_in(1, &mut frame);
    let mut large = SFixedPool::<Slot<u32>>::with_capacity_in(2, &mut frame);
    large.alloc(10).unwrap();
    let missing = large.alloc(20).unwrap();
    assert_eq!(missing.slot(), 1);
    assert_eq!(small.get(missing), None);
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| small.free(missing))).is_err()
    );
}

fn _slot_index_is_copy<T: PoolSlot>(pool: &mut SFixedPool<'_, T>, index: T::Index) {
    assert!(pool.get(index).is_none());
    assert!(pool.get(index).is_none());
}
