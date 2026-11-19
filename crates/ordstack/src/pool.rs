//! Stack-backed pools for values that you add and remove independently.
//!
//! Allocation returns an index. Freeing that index drops the value and
//! makes the slot available for reuse. Other values keep their indices
//! because the pool does not shift them.
//!
//! [`SPool`] grows in place and holds an exclusive borrow of its frame.
//! [`SFixedPool`] cannot grow, so its frame stays available for other
//! allocations. [`SPool::freeze`] keeps the slots and their indices in place
//! and releases the frame borrow.
//!
//! # Memory safety and index validity
//!
//! A lookup on a free slot returns `None`. Rust also prevents you from
//! freeing a slot while you still use a reference to its value. This
//! prevents memory use after free, but an index alone does not always
//! identify the value you meant.
//!
//! The default [`Slot`] uses only the slot number as its index. If the pool
//! reuses a slot, an old index can return the new value. The default works
//! well when callers stop using an index after they free it.
//!
//! # Why the slot type is customizable
//!
//! Each free slot stores a link to the next free slot. The pool stores the
//! head of this singly linked list, so the list needs no separate block of
//! memory. To store a value, the pool takes the first free slot in constant
//! time. If the list is empty, [`SPool`] appends a slot and grows if needed.
//!
//! [`PoolSlot`] lets you choose the slot layout. For example, you can reuse
//! the space that held the value to store the link. You can also keep a
//! generation counter in each slot and include it in the index. If reuse
//! changes the generation, [`PoolSlot::get`] can return `None` for an old
//! index instead of returning the new value.

use crate::frame::Frame;
use crate::slice::SSlice;
use crate::vec::SVec;

/// The storage and state encoding of one pool slot.
///
/// Free slots preserve a successor supplied by the pool. Occupied slots
/// preserve and later expose a live value. Implementations must report
/// `Some` from [`get`](Self::get) exactly while the slot is occupied and the
/// supplied index still names its allocation.
pub trait PoolSlot: Sized {
    /// The value stored in an occupied slot.
    type Value;

    /// The index returned for an allocation.
    ///
    /// It must fit in the caller's index type and may include information
    /// beyond the physical slot number, such as a generation.
    type Index: Copy;

    /// Creates a free slot whose optional successor is `next`.
    ///
    /// `None` marks the end of the free list. A `Some` number identifies a
    /// slot in the same pool.
    fn empty(next: Option<usize>) -> Self;

    /// Builds an occupied replacement for this free slot and returns its index.
    ///
    /// The pool calls this only on a free slot. `slot` is that slot's physical
    /// number. `value` is dropped if this method panics.
    fn occupy(&self, slot: usize, value: Self::Value) -> (Self, Self::Index);

    /// Builds a free replacement for this occupied slot, linked to `next`.
    ///
    /// The pool calls this only on an occupied slot. The old slot, including
    /// its value, is dropped after this method returns successfully.
    fn vacate(&self, next: Option<usize>) -> Self;

    /// Returns the successor of a free slot.
    ///
    /// # Panics
    /// If the slot is occupied.
    fn next(&self) -> Option<usize>;

    /// Returns the physical slot represented by `index`.
    ///
    /// The returned number is used to locate the slot before [`get`](Self::get)
    /// validates the rest of the index. It may be outside a particular pool.
    fn slot(index: Self::Index) -> usize;

    /// Returns the live value if this slot matches `index`.
    ///
    /// This returns `None` for a free slot and for an index whose allocation
    /// no longer exists.
    fn get(&self, index: Self::Index) -> Option<&Self::Value>;
}

/// The physical-slot index used by the default [`Slot`] representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Index(usize);

impl Index {
    /// Returns the physical slot number represented by this index.
    pub const fn slot(self) -> usize {
        self.0
    }
}

/// The default [`PoolSlot`] representation.
pub enum Slot<T> {
    /// A free slot and the next free slot in the pool.
    Empty {
        /// The successor, or `None` at the end.
        next: Option<usize>,
    },
    /// An occupied slot.
    Full {
        /// The live value.
        value: T,
    },
}

impl<T> PoolSlot for Slot<T> {
    type Value = T;
    type Index = Index;

    fn empty(next: Option<usize>) -> Self {
        Self::Empty { next }
    }

    fn occupy(&self, slot: usize, value: T) -> (Self, Index) {
        assert!(
            matches!(self, Self::Empty { .. }),
            "ordstack: cannot occupy a full pool slot"
        );
        (Self::Full { value }, Index(slot))
    }

    fn vacate(&self, next: Option<usize>) -> Self {
        assert!(
            matches!(self, Self::Full { .. }),
            "ordstack: cannot release an empty pool slot"
        );
        Self::Empty { next }
    }

    fn next(&self) -> Option<usize> {
        match self {
            Self::Empty { next } => *next,
            Self::Full { .. } => panic!("ordstack: occupied pool slot has no free-list link"),
        }
    }

    fn slot(index: Index) -> usize {
        index.0
    }

    fn get(&self, _index: Index) -> Option<&T> {
        match self {
            Self::Empty { .. } => None,
            Self::Full { value } => Some(value),
        }
    }
}

/// A fixed-capacity pool that leaves its frame available for allocation.
///
/// The slot block is frozen on the stack. New values can therefore allocate
/// from the same frame while the pool is used.
pub struct SFixedPool<'f, T: PoolSlot> {
    contents: SSlice<'f, T>,
    free_list_head: Option<usize>,
    len: usize,
}

fn take_free_slot<T: PoolSlot>(
    contents: &mut [T],
    free_list_head: &mut Option<usize>,
    len: &mut usize,
    slot: usize,
    value: T::Value,
) -> T::Index {
    let current = &contents[slot];
    let next = current.next();
    let (replacement, index) = current.occupy(slot, value);
    // Assignment drops the old slot before the list and count change.
    // Keep the old slot until both updates finish so a panic in Drop
    // leaves the pool consistent.
    let old = core::mem::replace(&mut contents[slot], replacement);
    *free_list_head = next;
    *len += 1;
    drop(old);
    index
}

fn free_slot<T: PoolSlot>(
    contents: &mut [T],
    free_list_head: &mut Option<usize>,
    len: &mut usize,
    index: T::Index,
) {
    let slot_number = T::slot(index);
    let slot = &mut contents[slot_number];
    assert!(
        slot.get(index).is_some(),
        "ordstack: pool index does not refer to an occupied slot"
    );
    let replacement = slot.vacate(*free_list_head);
    // If the value's destructor panics, the slot must already be on the
    // free list and the occupied count must already exclude it.
    let old = core::mem::replace(slot, replacement);
    *free_list_head = Some(slot_number);
    *len -= 1;
    drop(old);
}

impl<'f, T: PoolSlot> SFixedPool<'f, T> {
    /// Creates a fixed pool with `capacity` slots on `frame`.
    ///
    /// A zero-capacity pool is empty. Its `alloc` method always returns `None`.
    ///
    /// # Panics
    /// If the stack is exhausted, the capacity overflows, or a nonempty
    /// block needs alignment above 4096.
    pub fn with_capacity_in(capacity: usize, frame: &mut Frame<'f>) -> Self {
        let contents = frame.alloc_slice_from_iter((0..capacity).map(|index| {
            let next = index + 1;
            T::empty((next < capacity).then_some(next))
        }));
        Self {
            contents,
            free_list_head: (capacity > 0).then_some(0),
            len: 0,
        }
    }

    /// Returns the number of slots in this pool.
    pub fn capacity(&self) -> usize {
        self.contents.len()
    }

    /// Returns the number of occupied slots.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns whether every slot is free.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Allocates a value in a free slot, or drops it and returns `None` if
    /// the pool is full.
    pub fn alloc(&mut self, value: T::Value) -> Option<T::Index> {
        self.free_list_head.map(|slot| {
            take_free_slot(
                &mut self.contents,
                &mut self.free_list_head,
                &mut self.len,
                slot,
                value,
            )
        })
    }

    /// Frees the occupied slot at `index`, dropping its value.
    ///
    /// # Panics
    /// If `index` is outside this pool or does not refer to an occupied slot.
    pub fn free(&mut self, index: T::Index) {
        free_slot(
            &mut self.contents,
            &mut self.free_list_head,
            &mut self.len,
            index,
        );
    }

    /// Returns the value at `index` if the slot is occupied.
    pub fn get(&self, index: T::Index) -> Option<&T::Value> {
        self.contents
            .get(T::slot(index))
            .and_then(|slot| slot.get(index))
    }
}

/// A growable pool at the top of a frame.
///
/// This pool holds its frame exclusively, so it can grow in place. Call
/// [`freeze`](Self::freeze) to preserve all reserved slots and release the
/// frame borrow.
pub struct SPool<'v, 'f, T: PoolSlot> {
    contents: SVec<'v, 'f, T>,
    free_list_head: Option<usize>,
    len: usize,
}

impl<'v, 'f, T: PoolSlot> SPool<'v, 'f, T> {
    /// Creates a growable pool on `frame`, reserving `capacity` slots.
    ///
    /// A capacity of zero reserves no space. The first call to `alloc`
    /// grows the pool as needed.
    ///
    /// # Panics
    /// If the stack is exhausted, the capacity overflows, or a nonempty
    /// block needs alignment above 4096.
    pub fn with_capacity_in(capacity: usize, frame: &'v mut Frame<'f>) -> Self {
        Self {
            contents: SVec::with_capacity_in(capacity, frame),
            free_list_head: None,
            len: 0,
        }
    }

    /// Returns the number of slots available without another allocation.
    pub fn capacity(&self) -> usize {
        self.contents.capacity()
    }

    /// Returns the number of occupied slots.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns whether every allocated slot is free or none has been used.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns a frame above this pool's reserved capacity.
    pub fn above(&mut self) -> Frame<'_> {
        self.contents.above()
    }

    /// Allocates `value`, reusing a free slot or growing the pool.
    ///
    /// Existing indices remain valid across growth.
    ///
    /// # Panics
    /// If the stack is exhausted or the capacity overflows.
    pub fn alloc(&mut self, value: T::Value) -> T::Index {
        if let Some(slot) = self.free_list_head {
            return take_free_slot(
                &mut self.contents,
                &mut self.free_list_head,
                &mut self.len,
                slot,
                value,
            );
        }
        let slot = self.contents.len();
        let empty = T::empty(None);
        let (contents, index) = empty.occupy(slot, value);
        self.contents.push(contents);
        self.len += 1;
        index
    }

    /// Frees the occupied slot at `index`, dropping its value.
    ///
    /// # Panics
    /// If `index` is outside this pool or does not refer to an occupied slot.
    pub fn free(&mut self, index: T::Index) {
        free_slot(
            &mut self.contents,
            &mut self.free_list_head,
            &mut self.len,
            index,
        );
    }

    /// Returns the value at `index` if the slot is occupied.
    pub fn get(&self, index: T::Index) -> Option<&T::Value> {
        self.contents
            .get(T::slot(index))
            .and_then(|slot| slot.get(index))
    }

    fn initialize_spare_capacity(&mut self) {
        let capacity = self.capacity();
        while self.contents.len() < capacity {
            let index = self.contents.len();
            self.contents.push(T::empty(self.free_list_head));
            self.free_list_head = Some(index);
        }
    }

    /// Freezes this pool at its current capacity and releases the frame.
    ///
    /// Spare capacity becomes reusable free slots. Values and indices are
    /// preserved without moving.
    pub fn freeze(mut self) -> SFixedPool<'f, T> {
        self.initialize_spare_capacity();
        SFixedPool {
            contents: self.contents.freeze(),
            free_list_head: self.free_list_head,
            len: self.len,
        }
    }

    /// Moves this pool into `dst`, preserving its capacity, indices and values.
    ///
    /// The source buffer is left to be reclaimed. `dst` cannot be the frame
    /// this pool holds; this excludes the same-frame case at compile time.
    ///
    /// # Panics
    /// If `dst`'s stack is exhausted, the size overflows, or `T` needs
    /// alignment above 4096.
    pub fn freeze_into<'g>(mut self, dst: &mut Frame<'g>) -> SFixedPool<'g, T> {
        self.initialize_spare_capacity();
        SFixedPool {
            contents: self.contents.freeze_into(dst),
            free_list_head: self.free_list_head,
            len: self.len,
        }
    }
}
