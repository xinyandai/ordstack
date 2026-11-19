# Soundness of ordstack

This is an argument, not a mechanised proof. It states the invariants the
`unsafe` code relies on, argues that every public function preserves them,
lists the hazards considered, and ends with a table of every `unsafe` block
in the crate. The evidence beside it is described under
[Verification](#verification).

The claim argued here is the one the crate documentation makes: in safe code,
only the top block of a stack is ever grown, it grows in place, and memory is
reclaimed only when no live value can reach it. Nothing here is a claim about
speed or memory use, and the crate does not free frozen blocks one by one.
The optional `FrameAlloc` (feature `allocator-api2`) is the one exception to
the first half, by design: it serves collections this crate does not
control, and copies a block that is not on top when it grows. It still keeps
the second half; its argument is "The allocator adapter".

`SPool` uses `SVec` for storage and inherits its exclusive frame borrow.
`SFixedPool` uses `SSlice` and inherits its storage lifetime. Both pools
access slots through bounds-checked indexing. No unsafe code relies on the
free list or on `PoolSlot` returning the correct index.

## Definitions

- A stack has a region `[0, cap)` and an offset `top`. Offsets are relative
  to the region's base address.
- A **frame** `F` has an offset `F.end`. It is created by mutably borrowing a
  **parent**: the stack, another frame, or a growable container. Its
  **start** is the parent's end at that moment.
- A **growable container** `V` (`SVec`; `SString`, which wraps an
  `SVec<u8>`; and `SMap`) holds `&'v mut Frame<'f>`. Its **end** is its
  buffer's capacity end, or the frame's `end` while it has no buffer. A
  vector of a zero-size type never has a buffer. A map's buffer is its
  block: `usable(slots)` entries of `E = size_of::<Bucket<K, V>>()` bytes
  followed by `slots` index slots of 4 bytes, so its end is
  `start + usable(slots) * E + slots * 4`.
- A **frozen block** is the memory behind an `SBox`, `SSlice`, `SStr`,
  `SFixedVec`, `SFrozenMap` or `iter::IntoIter`. It carries the brand `'f` of the frame
  that allocated it. One exception: the `IntoIter<'v, T>` inside an
  `iter::VecIntoIter` is not a frozen block. Its memory is the consumed
  vector's unfrozen buffer, at or above `frame.end`; its brand `'v` is the
  loan of the frame that the `VecIntoIter` holds, so it is covered by B and
  H3(a), not by E (unsafe row 63).
- An object's **end** is `0` for the stack, `F.end` for a frame, and as
  above for a container.
- An object is **live** while safe code can still use it. Using a frame or
  container requires the borrow that created it to be alive, so a live
  object's parent is mutably borrowed. The live frames and containers of one
  stack therefore form a chain, each borrowing the one beneath, and ends do
  not decrease going up the chain: a child starts at its parent's end, and
  the parent's end cannot change while the child is live.

## Invariants

**I (reachability).** Every pointer into the region above an object's end
carries a lifetime that keeps that object mutably borrowed.

**T (top).** `top >=` the end of every live object.

**E (end is monotone).** `F.end` never decreases, and every frozen block
branded by `F` lies in `[F.start, F.end)`.

**B (buffer).** A container's buffer lies in `[frame.end, V.end)` and holds
`len` initialised values at its start.

**M (map block).** For an `SMap` with `slots > 0`: B holds for its block,
the first `len` entries are initialised, `len <= usable(slots)`, and the
`slots` index slots after the first `usable(slots)` entries are
initialised `u32`s. With `slots == 0` the map has no block, `len == 0`
and `ptr` is dangling and aligned. Which entries the slots name is a
correctness property only: slot values are used as indices into a slice
of length `len`, which is bounds-checked, and every probe is bounded by
`slots`, so a wrong index gives wrong answers or a panic, never undefined
behaviour. For an `SFrozenMap`, its block holds `len` initialised entries followed directly by `slots` initialised index slots, and is a frozen block (E); empty, it has no block and a dangling, aligned `ptr`.

## Why I holds

The only ways to obtain a pointer above an object's end are:

| Source | Lifetime on the result | What it borrows |
|---|---|---|
| `Stack::frame(&'a mut self)` | `Frame<'a>` | the stack, for `'a` |
| `Frame::sub(&'a mut self)` | `Frame<'a>` | the frame, for `'a` |
| `SVec::above(&'a mut self)`, `SString::above`, `SMap::above` | `Frame<'a>` | the container, for `'a` |
| `SVec::split_above(&'a mut self)`, `SString::split_above` | `Frame<'a>` (and `&'a mut [T]` / `&'a mut str`, which points below the container's end) | the container, for `'a` |
| `SVec::new_in(&'v mut Frame<'a>)`, `with_capacity_in`, `from_iter_in`, the `SString` constructors (and `sformat!`, which calls one), and the `SMap` constructors (`new_in`, `with_capacity_in`, `with_hasher_in`, `with_capacity_and_hasher_in`) | `SVec<'v, 'a, T>` / `SString<'v, 'a>` / `SMap<'v, 'a, K, V, S>` | the frame, for `'v`; `'a` already borrows the frame's parent |
| `SVec<'v, 'a, T>::into_iter` (by value) | `VecIntoIter<'v, 'a, T>`, which keeps the frame borrow | the frame, for `'v`, as the vector did |
| `Frame<'a>::alloc`, `try_alloc`, `alloc_with`, `alloc_slice_copy`, `alloc_slice_clone`, `alloc_slice_from_iter`, `alloc_str`, `SFixedVec::with_capacity_in` | handle branded `'a` | nothing new: `'a` already borrows the parent. The block is below the frame's own end, so I asks nothing about the frame |
| `SVec<'v, 'a, T>::freeze`, `SString::freeze`, `SMap::freeze` | `SSlice<'a, T>` / `SStr<'a>` / `SFrozenMap<'a, K, V, S>` | nothing new |
| `SVec::freeze_into(self, &mut Frame<'g>)`, `SString::freeze_into`, `SMap::freeze_into` | `SSlice<'g, T>` / `SStr<'g>` / `SFrozenMap<'g, K, V, S>` | nothing new: `'g` already borrows the destination's parent |
| `SFixedVec::into_slice`, `SFixedVec::into_iter`, `SSlice::into_iter` | same brand as the input | nothing new |
| `Frame::allocator(&'a mut self)` (feature `allocator-api2`) | `FrameAlloc<'a, 'f>`, and through its `Allocator` methods raw blocks above the frame's end | the frame, for `'a` |
| `SBox::leak`, `SSlice::leak`, `SStr::leak` | `&'a mut` | nothing new |
| `Deref` / `DerefMut` on any handle | a reference bounded by the borrow of the handle | the handle, whose type carries the brand |

Every handle type is covariant or invariant in its brand (H4 lists which),
so a brand can be shortened at most, never lengthened, and a shortened brand
is still outlived by the borrow that created it. `Frame` is neither `Clone` nor `Copy`. No function returns a
pointer without a brand, except the `Allocator` methods of `FrameAlloc`,
which return raw `NonNull<[u8]>`: using one takes `unsafe` code bound by the
trait's contract, under which a block is valid only while the allocator is
alive, so only while the borrow of the frame is. `Frame::end_offset`, `Stack::extent`,
`Stack::high_water` and `Stack::capacity` return plain integers, and no safe
function turns an integer back into a pointer.

## Why T holds

`top` is written only in `stack.rs`, by `try_bump` and `try_extend`, which
raise it, and by `reset_to`, which lowers it. Raising `top` cannot break T.
The callers that create or enlarge an object set its end to at most the new
`top`: `Frame::try_alloc_array` sets `end` to the end of the block just
bumped, and `SVec::try_grow_to` and `SMap::try_grow_to` set the capacity
whose end is the new `top`. `FrameAlloc` (feature `allocator-api2`) raises
`top` the same way when it allocates, or grows its top block in place.

Except in `FrameAlloc::deallocate` and `FrameAlloc::shrink` (the last two
rows below, argued in "The allocator adapter"), `reset_to(off)` is called
only by a method that holds `&mut` (or ownership) of an object `X`, with `off` equal to `X`'s end as it is after the call. With
`&mut X` held, no child of `X` is live, so `X` is the top of the chain and
every other live object has its end at or below `X`'s. So T holds after the
reset.

The call sites, all of them:

| Caller | `off` | `X`'s end afterwards |
|---|---|---|
| `Stack::frame`, `Stack::reset`, `Stack::trim` | `0` | `0` |
| `Frame::sub`, `reset_above`, `trim_above`, `try_alloc_array` | `self.end` | `self.end` (then raised by a successful bump) |
| `SVec::try_grow_to`, `above`, `split_above` | the vector's end | unchanged (then raised by a successful growth) |
| `SVec::shrink_to_fit`, `len > 0` | buffer start + `len` elements | the same: `cap := len` |
| `SVec::shrink_to_fit`, `len == 0` | `frame.end` | `frame.end`: the buffer is given up, `cap := 0` |
| `SVec::freeze`, `len > 0` | buffer start + `len` elements | the vector is consumed; `frame.end := off` |
| `SVec::freeze`, `len == 0`, buffer allocated | `frame.end` | the vector is consumed; `frame.end` unchanged |
| `SMap::try_grow_to`, `above` | the map's block end | unchanged (then raised by a successful growth) |
| `SMap::freeze`, `len > 0` | block start + `len` entries + the compact index | the map is consumed; `frame.end := off` |
| `SMap::freeze`, `len == 0`, block allocated | `frame.end` | the map is consumed; `frame.end` unchanged |
| `Frame::allocator` | `self.end` | `self.end`; the adapter's blocks lie above it and borrow the adapter |
| `FrameAlloc::deallocate`, block on top | the block's start | not an object end: see "The allocator adapter" |
| `FrameAlloc::shrink`, block on top and aligned | block start + new size | not an object end: see "The allocator adapter" |

`reset_to` checks `off <= top` with a `debug_assert!` and does nothing when
`top` is already at or below `off`.

## Why reset on reacquire is sound

In every row above except the two `FrameAlloc` rows (see "The allocator
adapter") the caller holds `&mut` to `X`, or owns it. That means no
borrow of `X` is live. By I, every pointer above `X`'s end needs such a
borrow. So nothing above `X`'s end is reachable.

In the `SVec::shrink_to_fit` and `SVec::freeze` rows `off` is below `X`'s old end, inside the vector's own
buffer or at its frame's end. The extra range is `[off, old end)`. It holds
the vector's own spare capacity (or, when `len == 0`, its whole empty
buffer) and alignment padding before the buffer, and nothing else: by B the
buffer starts at or above `frame.end`, the vector holds its frame
exclusively so nothing else was allocated from the frame since the buffer
was placed, and by E every frozen block of the frame is below `frame.end`.
No initialised element lies in that range. So lowering `top` to `off`
invalidates no reachable memory.

Memory below `off` is unaffected: the reset never lowers `top` below the
caller's own end, and every object beneath the caller has its end at or
below the caller's.

## Why growth is in place

`SVec::try_grow_to` holds `&mut self`, resets `top` to the vector's end, and
then either bumps a fresh buffer (no buffer yet) or extends the existing one.
After the reset the buffer ends exactly at `top`, so extending it overlaps
nothing. `Stack::try_extend` checks this with `assert_eq!(old_end, top)` in
every build, debug and release; if the argument here is right the assertion
never fires. Apart from the lazy reset, which can only lower `top`, it is
the only place in a release build where the library compares a block's end
against `top`, and it guards against a bug in this crate rather than choosing between a
fast and a slow path: there is no path that moves a buffer in the crate's own containers.
`FrameAlloc` also compares a block's end with `top`, to choose between
growing in place and copying; see "The allocator adapter".

## The allocator adapter

With the `allocator-api2` feature, `Frame::allocator(&mut self)` resets to
the frame's end and returns `FrameAlloc`, which holds the `&mut Frame`. Its
blocks are allocated by `try_bump` above `top` and lie above the frame's
end; collections reach them only through `&FrameAlloc` or by owning the
`FrameAlloc`, both of which keep the frame mutably borrowed, so I holds.
While the adapter lives nothing else allocates on the stack: the frame is
borrowed by the adapter and every frame below is frozen by the borrows that
made this one. So the adapter's blocks are the only objects above the
frame's end, and every one of them ends at or below `top`.

- **Disjoint.** A fresh block starts at or above `top`. A block grows in
  place only when it ends at `top`, so it grows into space no live block
  uses. A zero-size block occupies nothing.
- **T.** `deallocate` and `shrink` lower `top` only for a block that ends
  at `top`, to that block's start or new end; every other live block
  of non-zero size ends at or below the block's start, so `top` stays `>=` every live end. (Two
  blocks can end at `top` only if one of them has size zero.)
- **E.** The frame's `end` is not written. The adapter's blocks are not
  frozen blocks and carry no `'f` brand.
- **The trait's contract.** A block stays valid until it is deallocated,
  shrunk or grown (each returns the block that replaces it), or until a
  reset; no reset can happen while the adapter is alive or borrowed.
  `allocate`, `grow` and `shrink` return blocks of the requested size and
  alignment: `try_bump` aligns the start, and the in-place paths are taken
  only when the existing start is aligned for the new layout. Failures
  return `AllocError` and leave `top` unchanged (`try_bump`, `try_extend`).
- **Zero sizes.** `top_block` answers "not on top" for every zero-size
  block, so a zero-size block never moves `top` when it is freed and is
  never extended in place. This also covers pointers that did not come
  from `try_bump`: allocator-api2's `Box` hands out `NonNull::dangling()`
  for zero-size values and still passes it to `deallocate`
  (`tests/allocator.rs::dangling_zero_size_blocks_are_ignored`).
- **H3.** No user code runs inside the adapter, so nothing re-enters it
  mid-operation.

## What a failed allocation leaves behind

A failed allocation or growth changes no object: no length, capacity,
buffer pointer or frame end is written, and no stored value is dropped or
moved. The value that was to be added is handed back by `SVec::try_push`
and dropped by `Frame::try_alloc` and by the panicking methods.

It may still lower `top`. `Frame::try_alloc_array` and `SVec::try_grow_to`
run the reset to the caller's own end first and the fallible step
(`try_bump`, `try_extend`, or the overflow check on the new end) second. So
a failure can reclaim unreachable space above the caller, exactly as a
successful call would have, and `Stack::extent` can be smaller afterwards.
That reset is sound for the reason given above whether or not the next step
succeeds. `try_bump` and `try_extend` themselves write `top` only on
success. `SVec::try_reserve` tries a doubled capacity and then the exact
one; both attempts follow the same rule.
(`tests/frozen.rs::failed_allocation_changes_no_object_but_may_reclaim_above`
pins this behaviour.)

## Why frozen blocks stay valid for their brand

By E a block branded by `F` lies below `F.end`. `top` can be lowered below
`F.end` only by a method holding `&mut` to an object beneath `F`, or the
region can be unmapped by dropping the stack. Every object beneath `F`, and
the stack, is mutably borrowed for at least `F`'s brand. So the block is
neither reclaimed nor unmapped while the brand is alive. `F` itself never
resets below its own `end`.

E holds because `F.end` is written in three places. `try_alloc_array` sets
it to the end of a block bumped at or above the old `end`. `SVec::freeze`
sets it to the end of the vector's initialised elements, which by B is above
the old `end`; `freeze` of a vector with no elements or no buffer leaves it
unchanged. `SMap::freeze` sets it to the end of the compacted block (the
entries and the new index), which lies inside the map's block and so, by B,
above the old `end`; `freeze` of a map with no entries leaves it unchanged.

Blocks do not overlap: `try_alloc_array` resets `top` to `F.end` (above
every earlier block of `F`) and bumps from there; a vector's buffer is
bumped the same way while the vector holds the frame exclusively.

`Stack::trim` and `Frame::trim_above` discard pages (`madvise`) only at or
above the offset they have just reset `top` to, rounded up to a page
boundary, so only unreachable memory loses its contents. The range stays
mapped. Memory handed out again later is written before it is read: the
`Frame::alloc*` methods fill their block before returning a handle, and
`SVec` and `SFixedVec`, whose buffers start uninitialised, read only the
first `len` slots, which they have written.

## Hazards

### H1. `mem::forget` and `ManuallyDrop`

No destructor is relied on for memory safety. No handle, frame or container
reclaims memory in `Drop`; their destructors only drop values. The one
destructor that releases memory is the stack's, which unmaps the whole
region, and the stack cannot be dropped while anything borrows it
(`tests/ui/fail/06_*`, `08_*`).

- Forgetting a frame or container ends its borrow. Its space is reclaimed
  when the parent next resets (see the table under "Why T holds"), or when
  the stack is dropped.
- Forgetting a frozen handle leaks its value. The brand's borrow is
  unaffected.
- Forgetting a `Stack` leaks the mapping.
- `ManuallyDrop<SVec<..>>` removes the destructor's use of the vector at
  scope end. This lets a block from `split_above`'s frame be stored in the
  vector's own element (`tests/ui/fail/12_*` and its passing twin), which
  forces the borrow of the vector to last as long as its element type. The
  vector can then never be used again, so it can never reset below the
  stored block; and the frame beneath stays borrowed for as long as the
  element is reachable (the rejected line in `12_*` is exactly a use of that
  frame while the element is still read). The stored block is leaked, not
  dangling. Without `ManuallyDrop` the store is rejected outright
  (`tests/ui/fail/11_*`).
- `SBox::leak(g.alloc(vec))`, with the vector moved into a box on some other
  frame `g` and the box leaked, is another way to hold an `SVec` whose
  destructor never runs. The same argument covers it: the vector is still a
  value that must be borrowed to be used, and the frame it holds stays
  borrowed for as long as it is reachable.

### H2. Drop check

`SBox`, `SSlice`, `SFixedVec`, `SVec` and `iter::IntoIter` implement `Drop`
without `#[may_dangle]`; `iter::VecIntoIter` inherits the obligation through
its `IntoIter<'v, T>` field, so the frame loan `'v` it holds lasts until it
is dropped, not merely until its last use. (`SStr` and `SString` wrap an `SSlice<u8>` and an
`SVec<u8>`.) The compiler therefore requires every lifetime in `T`, and the
brand, to be alive when the handle is dropped. A destructor of `T` that
reads a frozen block of the same or a lower frame reads memory that is still
allocated, by the previous section. `tests/ui/fail/10_*` and `11_*` show the
compiler rejecting the stores that would otherwise let a lower container own
a block from a frame above it.

### H3. User code inside the library: unwinding and re-entrancy

User code runs inside the library in: element destructors, `Clone::clone`
(`alloc_slice_clone`), iterators (`Extend::extend`, `from_iter_in`, the
`SString` `Extend` impls), the `retain` predicate, the `alloc_with` closure,
the comparison, hashing and formatting impls that delegate to the
elements, the `Hash` and `Eq` impls of map keys and the `BuildHasher` of a map
(`SMap::insert`, every lookup and keyed removal, `entry`), `Extend` for `SMap`,
and the `or_insert_with` closure.

Re-entrancy. Three different situations, which must not be conflated:

(a) Growable containers. `SVec`, `SString` and `SMap` (and `iter::VecIntoIter`,
which keeps a consumed vector's frame and runs only element destructors)
hold `&mut Frame` for as long as they exist, and the methods that run user code (`retain`, `extend`,
`truncate`, `clear`, `Drop`) borrow the container itself. User code run from
them cannot name the container in its intermediate state, and cannot reach
its frame, or any frame beneath it, because those stay mutably borrowed.
`Frame::alloc_slice_clone` and `Frame::alloc_with` are methods of the frame:
they run `Clone::clone` and the closure while holding `&mut self`, so the
callee cannot reach that frame either (a `RefCell<Frame>` does not help,
since the caller already holds its `borrow_mut`). `alloc_with` runs the
closure before anything is reserved; `alloc_slice_clone` has already raised
`frame.end` past the whole block when the first `clone` runs, so even a
panic leaves the frame consistent. `SMap` runs the hasher, `Hash` and `Eq` before it changes anything (`insert` and `entry` hash and probe first; `push` then runs no user code), and its growth rebuilds the index from stored hashes, so no user code runs while the index is being rebuilt.

(b) Frozen handles (`SBox`, `SSlice`, `SStr`, `SFixedVec`, `SFrozenMap`, `IntoIter`; not
the `IntoIter` inside a `VecIntoIter`, which is (a)). These do not borrow
their frame, so an element destructor *can* use the frame that
allocated the very block it sits in (for example through a
`RefCell<Frame>`), and may call `alloc`, `sub`, `reset_above`, `trim_above`
or `end_offset` on it. This is sound, and the earlier claim that it was
impossible was wrong. The reason is that every frozen block lies below
`frame.end` (E), and every operation reachable through a `&mut Frame` acts
only at or above `frame.end`:

- `try_alloc_array` calls `reset_to(self.end)` and then `try_bump`, which
  starts at `top >= end` after the reset (it can only round upwards), and
  sets `end` to the end of the new block, so `end` never decreases;
- `sub` and `reset_above` call `reset_to(self.end)`; a sub-frame starts at
  `end`, so its blocks lie at or above it, and its own reset only goes down
  to its own `end`;
- `trim_above` calls `reset_to(end)` and `release_above(end)`; the
  `release_pages` helper rounds the start *up* to a page boundary, so only
  pages that lie entirely at or above `end` are discarded;
- `end_offset` only reads;
- building an `SVec`, `SString` or `SFixedVec` on the frame goes through the
  same reset and bump; `freeze`, `into_slice` and `freeze_into` (with the
  frame as destination) only raise `end`;
- `mem::swap`, `mem::replace` and `Option::take` on the frame value move the
  stack reference and `end` together, so each frame still resets only to its
  own `end` on its own stack.

A frozen map's lookups run the hasher, `Hash` and `Eq`, which may therefore use the map's own frame; the map's block lies below `frame.end` like any frozen block. `tests/map.rs::a_hash_that_uses_the_frame_cannot_disturb_a_frozen_map` runs allocation, `reset_above` and `trim_above` from a key's `Hash` during lookups, under Miri as well.

This follows from `frame.rs`, `stack.rs`, `vec.rs` and `region.rs`. (`SStr`
is listed for completeness; it holds bytes and runs no user code.) A destructor
that is itself running inside a sub-frame's handle can reach only that
sub-frame (its parent is mutably borrowed by it, and a `RefCell` guard on
the parent stays alive as long as the sub-frame does). The frozen block being
destroyed is not read after its destructor starts, and the owner has already
lowered its count (see the table), so a re-entrant allocation cannot observe
or free it. `tests/reentrant.rs` runs the `alloc`, `sub`, `reset_above`
and `trim_above` cases, including under Miri with both borrow models. The
remaining operations, and the `RefCell` guard on a parent frame, were
exercised by the second review round (`docs/reviews/DISPOSITION.md`) and
are not in the test suite.

(c) The comparison, hashing and formatting impls take `&self` and delegate
to initialised shared references. They can run arbitrary code, including
interior mutation of the elements, but expose no intermediate allocation
state: nothing in them is mid-update.

| Site | State when user code runs | On panic |
|---|---|---|
| `drop_back_to_front` (every multi-element destructor, `truncate`, `clear`) | the owner's `len` is already lowered (or, for `IntoIter`, its range emptied) | a guard drops the rest; a second panic aborts, as in `std` |
| `SBox::drop` | the box is being dropped | the value counts as dropped; nothing else to undo |
| `alloc_slice_clone` | block reserved and `frame.end` raised past it; `n` slots initialised | a guard drops the `n` clones back to front; the space stays with the frame |
| `Extend::extend`, `from_iter_in` | vector consistent between pushes | elements pushed so far remain in the vector (`from_iter_in` then drops the vector and them) |
| `retain`, predicate | `len == 0`; kept prefix and unvisited tail tracked by a guard; the value under test is still in the unvisited tail | the guard closes the gap and restores `len` |
| `retain`, destructor of a rejected element | as above, with the read position already past the element | as above; the element is not dropped twice |
| `alloc_with` | nothing reserved: the closure runs before the allocation | nothing to undo |
| `freeze_into` when the destination is exhausted (the panic is the library's own) | the source vector is intact | the vector is dropped with its elements |
| `SMap::insert`, lookups, `swap_remove`, `shift_remove` (hasher, `Hash`, `Eq`) | nothing changed yet: the key is hashed and the index probed before any write | nothing to undo |
| `SMap::clear`, `Drop` for `SMap` (entry destructors) | `len := 0` and, for `clear`, the index already emptied | `drop_back_to_front`'s guard drops the rest; the map is empty and consistent |
| `Extend` for `SMap` | map consistent between inserts | entries inserted so far remain |
| `SMap::freeze_into` when the destination is exhausted | the source map is intact | the map is dropped with its entries |
| `Entry::or_insert_with` closure | the key was probed and found absent; nothing reserved yet | nothing to undo; the key is dropped with the entry |

In every case `top` and every `end` are consistent, because user code never
runs between a change to `top` and the matching change to a container's or
frame's fields. `tests/panics.rs` exercises the destructor, `clone`,
`retain`, `alloc_with` and `extend` rows.

### H4. Variance

| Type | `'v` | `'f` | `T` |
|---|---|---|---|
| `Frame<'f>` | – | covariant | – |
| `SBox<'f, T>`, `SSlice<'f, T>`, `SFixedVec<'f, T>`, `IntoIter<'f, T>` | – | covariant | covariant |
| `SStr<'f>` | – | covariant | – |
| `SVec<'v, 'f, T>` | covariant | invariant | covariant |
| `VecIntoIter<'v, 'f, T>` | covariant | invariant | covariant |
| `SString<'v, 'f>` | covariant | invariant | – |
| `SMap<'v, 'f, K, V, S>` | covariant | invariant | covariant in `K`, `V`, `S` |
| `SFrozenMap<'f, K, V, S>` | – | covariant | covariant in `K`, `V`, `S` |
| `map::Iter<'a, K, V>`, `Keys`, `Values` | – | – | covariant in `K`, `V` (they wrap `slice::Iter`) |
| `map::IterMut<'a, K, V>`, `ValuesMut` | – | – | invariant in `K`, `V` (they wrap `slice::IterMut`) |

`Frame<'f>` holds `&'f Stack`. The frozen handles hold a `NonNull<T>` and
`PhantomData<(&'f (), T)>` (`SFrozenMap` holds `PhantomData<(&'f (), Bucket<K, V>)>`). Shortening a brand is sound (the block is valid
for the longer lifetime). `SVec` must be invariant in `'f` because it holds
`&mut Frame<'f>` and `freeze` returns that brand; it gets this from the
reference `&'v mut Frame<'f>`. Covariance in `T` is the same as `Vec<T>`'s
and `Box<T>`'s: the handles own their values. `SMap` gets its invariance in `'f` the same way as `SVec`, from `&'v mut Frame<'f>`; it owns its entries (`PhantomData<Bucket<K, V>>`).

### H5. Swapping frames

`mem::swap` on two `&mut Frame<'x>` is safe: each frame carries its own
stack reference and `end`, so the swapped values are still consistent, and
both were already borrowing their parents for `'x`. Two frames at different
depths of one stack cannot be named at once, because the deeper one borrows
the shallower. A sub-frame cannot be moved into a longer-lived slot, because
its brand is the borrow of its parent.

### H6. Zero-size types and alignment

A request of zero bytes (a zero-size `T`, or zero elements) never reaches
`Stack::try_bump`: `Frame::try_alloc_array` returns `NonNull::dangling()`,
which is aligned for `T`, before touching the stack. Such a request does not
reset `top`, does not move `end`, and cannot fail for lack of room or for
alignment. In particular a zero-size `T` whose alignment is above 4096
succeeds, with an aligned dangling pointer
(`tests/frozen.rs::zero_size_value_with_alignment_above_the_limit_succeeds`).
Through `FrameAlloc` (feature `allocator-api2`) a zero-size layout does
reach `try_bump`: it gets a zero-byte block at `top`, so an alignment above
4096 fails, and alignment padding moves `top` up until the adapter's memory
is reclaimed.

`SVec` gives a zero-size `T` capacity `usize::MAX` and never allocates; its
`len` cannot overflow because `try_reserve` checks `len + additional` and
reports `CapacityOverflow`.

For a block of non-zero size, `try_bump` refuses alignment above 4096 with
`AlignTooLarge`. That check comes after the caller's reset, so `top` may
already have been lowered (see "What a failed allocation leaves behind"); no
object changes.

Offsets are aligned, not addresses, so alignment of the address depends on
the base. `Region::reserve` asserts `base % 4096 == 0` with a real
`assert_eq!` in every build, so a platform whose mapping is less aligned
panics when the stack is created instead of handing out misaligned blocks.
On Unix the base is an `mmap` result, and `os::page_size` is at least 4096
whatever the system reports. Under Miri the base
is a heap block requested with alignment 4096. A zero-length region uses the
address 4096 as a base that is never dereferenced, because every non-zero
request on it fails.

A map entry is never zero-size and its alignment is at least that of
`u64` (8, or 4 on some 32-bit targets), because it holds the `u64` hash; so a map of zero-size keys and values still bumps a
real block, and the index that follows `n` entries is 4-aligned. An entry
whose `K` or `V` needs alignment above 4096 fails with `AlignTooLarge`.

### H7. Arithmetic

Every size computation that can overflow is checked (`checked_mul`,
`checked_add`) and reported as `CapacityOverflow`: the byte size of an
array, the aligned start and the end in `try_bump`, the new end in
`try_grow_to`, and `len + additional` in `try_reserve`. The region is at
most `isize::MAX` minus one page, so every block satisfies the size limit of
`slice::from_raw_parts`. Offsets within a live buffer
(`cap * size_of::<T>()`, `len * size_of::<T>()`) cannot overflow because the
buffer exists. `Stack::ptr_at` checks `off <= region length` with an
`assert!` in every build before it does pointer arithmetic. For maps, `slots_for` refuses more than `u32::MAX - 1` entries and doubles with `checked_mul`; `block_bytes` multiplies and adds with `checked_*`; the new end in `try_grow_to` is a `checked_add`. `usable(slots) * E + slots * 4` for an existing block, and the compacted size in `freeze` (which is smaller), cannot overflow because the block exists. An entry index plus one fits a `u32` because `len <= u32::MAX - 1`: `capacity()` is capped at that limit even when the largest index has room for more, so a push past it reaches `slots_for`, which refuses it (unit test `the_entry_limit_holds_with_spare_capacity`).

### H8. Threads

`Stack` contains `Cell` and `NonNull`; every handle contains `NonNull` or a
reference to a `Stack`. None is `Send` or `Sync`, and there is no `unsafe
impl` of either (`tests/ui/fail/13_*`, `14_*`). `FrameAlloc` holds
`&mut Frame`, which holds `&Stack`, so it is neither `Send` nor `Sync`
(`tests/ui/fail/23_*`, `24_*`). Its `unsafe impl` is of
`allocator_api2::alloc::Allocator`, not of `Send` or `Sync`.

### H9. The debug shadow

`debug.rs` is a checker, not part of the argument, and contains no `unsafe`.
In builds with `debug_assertions` it keeps a thread-local table of regions
and their `top`, and handles assert on access that their block lies below
`top`. A thread-local table is correct because a stack cannot leave its
thread (H8). Release builds compile it to nothing. `iter::IntoIter` does
not consult it.

### H10. Internal contracts that are not `unsafe fn`

Four crate-private functions in `stack.rs` are safe functions that the
`unsafe` code elsewhere depends on:

| Function | Contract | How it is enforced |
|---|---|---|
| `Stack::ptr_at(off)` | `off <= region length` | `assert!`, every build |
| `Stack::try_extend(old_end, new_end)` | `old_end == top`, `new_end >= old_end` | `assert_eq!` / `assert!`, every build |
| `Stack::offset_of(p)` | `p` points into this stack's region | `debug_assert!` only. Its callers pass a buffer pointer that came from `ptr_at` on the same stack, and only when the vector has a buffer |
| `Stack::release_above(off)` | `off >= top` | `debug_assert!` only. Both callers call `reset_to(off)` on the line before |

`Stack::reset_to(off)` has the contract argued under "Why T holds";
its `debug_assert!` covers only `off <= top`.

## Per-function table

One row for each of the 129 public functions (`grep -n "pub fn\|pub const fn"
crates/ordstack/src/*.rs`), then the trait impls. "Resets" means
`reset_to(own end)`, justified under "Why reset on reacquire is sound".

### `Stack`

| Function | Takes | Preserves I, T, E, B because |
|---|---|---|
| `Stack::new` | – | calls `with_reserve`; a new stack has `top = 0` and no objects |
| `Stack::with_reserve` | – | as `try_with_reserve`, panicking on error |
| `Stack::try_with_reserve` | – | maps a fresh region or returns an error; `top = 0` |
| `Stack::frame` | `&mut Stack` | nothing on the stack is reachable; `top := 0`; the new frame has `end = 0` and borrows the stack |
| `Stack::reset` | `&mut Stack` | nothing on the stack is reachable; `top := 0` |
| `Stack::trim` | `&mut Stack` | as `reset`, then discards pages from offset 0, all unreachable |
| `Stack::capacity` | `&Stack` | reads the region length |
| `Stack::extent` | `&Stack` | reads `top` |
| `Stack::high_water` | `&Stack` | reads the recorded maximum of `top` |

### `Frame`

| Function | Takes | Preserves I, T, E, B because |
|---|---|---|
| `Frame::sub` | `&mut Frame` | resets; the new frame starts at this frame's end and borrows it |
| `Frame::reset_above` | `&mut Frame` | resets |
| `Frame::trim_above` | `&mut Frame` | resets, then discards whole pages at or above `end`, all unreachable |
| `Frame::end_offset` | `&Frame` | reads `end` |
| `Frame::stack` | `&Frame` | returns `&Stack` bounded by the borrow of the frame; through `&Stack` only `capacity`, `extent`, `high_water` and `Debug` are reachable, and they read `Cell`s |
| `Frame::alloc_slice_from_iter` | `&mut Frame<'f>` | `SVec::from_iter_in` on this frame, then `SVec::freeze`; no code of its own touches the stack |
| `Frame::alloc_slice_copy` | `&mut Frame<'f>` | block from `try_alloc_array` (resets, bumps, raises `end` past the block: E); filled before the handle exists |
| `Frame::alloc_slice_clone` | `&mut Frame<'f>` | as above; clones written one by one under a guard (H3) |
| `Frame::alloc_str` | `&mut Frame<'f>` | `alloc_slice_copy` of the bytes of a `str` |
| `Frame::alloc` | `&mut Frame<'f>` | `try_alloc`, panicking on error |
| `Frame::try_alloc` | `&mut Frame<'f>` | block for one `T` from `try_alloc_array`; on error nothing but `top` has changed |
| `Frame::alloc_with` | `&mut Frame<'f>` | runs the closure, then `alloc` |
| `Frame::allocator` | `&mut Frame<'f>` | resets; the adapter holds the frame (see "The allocator adapter") |

### `SBox`, `SSlice`, `SStr`

| Function | Takes | Preserves I, T, E, B because |
|---|---|---|
| `SBox::into_inner` | `SBox` by value | reads the value out and forgets the box; no stack state changes |
| `SBox::leak` | `SBox<'f, T>` by value, `T: 'f` | returns `&'f mut T` to a block valid for `'f`; the box is forgotten |
| `SSlice::empty` | – | dangling pointer, length 0; no block |
| `SSlice::leak` | `SSlice<'f, T>` by value, `T: 'f` | as `SBox::leak` |
| `SStr::empty` | – | wraps `SSlice::empty` |
| `SStr::as_str` | `&SStr` | `Deref` |
| `SStr::leak` | `SStr<'f>` by value | `SSlice::leak` of the bytes |

### `SFixedVec`

| Function | Takes | Preserves I, T, E, B because |
|---|---|---|
| `SFixedVec::with_capacity_in` | `&mut Frame<'f>` | block for `cap` values from `try_alloc_array`; `len = 0`; does not keep the frame |
| `SFixedVec::capacity` | `&SFixedVec` | reads `cap` |
| `SFixedVec::is_full` | `&SFixedVec` | reads `len`, `cap` |
| `SFixedVec::push` | `&mut SFixedVec` | `try_push`, panicking when full |
| `SFixedVec::try_push` | `&mut SFixedVec` | writes slot `len < cap` inside its own block; never touches the stack |
| `SFixedVec::pop` | `&mut SFixedVec` | lowers `len`, reads the slot out |
| `SFixedVec::into_slice` | `SFixedVec<'f, T>` by value | same block, same brand, first `len` values; spare capacity stays below `frame.end` unused |

### `SVec`

| Function | Takes | Preserves I, T, E, B because |
|---|---|---|
| `SVec::new_in` | `&'v mut Frame<'f>` | no buffer; end is `frame.end`; holds the frame for `'v` |
| `SVec::with_capacity_in` | `&'v mut Frame<'f>` | `new_in`, then `try_grow_to` |
| `SVec::from_iter_in` | `&'v mut Frame<'f>` | `new_in`, then `extend` |
| `SVec::reserve` | `&mut SVec` | `try_reserve`, panicking on error |
| `SVec::try_reserve` | `&mut SVec` | grows only through `try_grow_to` (resets, then bumps or extends at `top`: B); on error no field changes |
| `SVec::capacity` | `&SVec` | reads `cap` |
| `SVec::push` | `&mut SVec` | `try_push`, panicking on error |
| `SVec::try_push` | `&mut SVec` | grows only through `try_reserve`; writes slot `len < cap` |
| `SVec::pop` | `&mut SVec` | lowers `len`, reads the slot out; never touches the stack |
| `SVec::extend_from_slice` | `&mut SVec` | `reserve`, then one copy into spare capacity |
| `SVec::insert` | `&mut SVec` | `reserve(1)`, then shifts inside the buffer |
| `SVec::remove` | `&mut SVec` | shifts inside the buffer |
| `SVec::swap_remove` | `&mut SVec` | moves one element inside the buffer |
| `SVec::truncate` | `&mut SVec` | lowers `len`, then drops the tail back to front |
| `SVec::clear` | `&mut SVec` | `truncate(0)` |
| `SVec::retain` | `&mut SVec` | compacts inside the buffer under a guard (H3) |
| `SVec::shrink_to_fit` | `&mut SVec` | lowers `top` into its own spare capacity and `cap` with it (or gives up an empty buffer) |
| `SVec::above` | `&mut SVec` | resets to the vector's end; the new frame starts there and borrows the vector |
| `SVec::split_above` | `&mut SVec` | as `above`; the element slice is below the vector's end, the frame hands out memory only at or above it, so the two are disjoint |
| `SVec::freeze` | `SVec<'v, 'f, T>` by value | lowers `top` to the end of the elements; raises `frame.end` to the same offset (E); returns the block with the frame's brand |
| `SVec::freeze_into` | `SVec` by value, `&mut Frame<'g>` | block allocated by `dst` like any slice; one bitwise copy; source `len := 0` so each value is owned once; the source buffer is left for lazy reclamation. `dst` cannot be the vector's own frame or one beneath it (they are mutably borrowed by the vector) nor one above it (that would borrow the vector), so it belongs to another stack and the fresh block cannot overlap the source buffer |

### `SString`

| Function | Takes | Preserves I, T, E, B because |
|---|---|---|
| `SString::new_in` | `&'v mut Frame<'f>` | `SVec::new_in` |
| `SString::with_capacity_in` | `&'v mut Frame<'f>` | `SVec::with_capacity_in` |
| `SString::from_str_in` | `&'v mut Frame<'f>` | `with_capacity_in`, then `push_str` |
| `SString::push` | `&mut SString` | `push_str` of the encoded character |
| `SString::push_str` | `&mut SString` | `SVec::extend_from_slice` of whole `str` bytes: stays UTF-8 |
| `SString::pop` | `&mut SString` | truncates by the last character's length: stays UTF-8 |
| `SString::truncate` | `&mut SString` | asserts a character boundary first, then `SVec::truncate` |
| `SString::clear` | `&mut SString` | `SVec::clear` |
| `SString::reserve` | `&mut SString` | `SVec::reserve` |
| `SString::try_reserve` | `&mut SString` | `SVec::try_reserve` |
| `SString::capacity` | `&SString` | `SVec::capacity` |
| `SString::above` | `&mut SString` | `SVec::above` |
| `SString::split_above` | `&mut SString` | `SVec::split_above`; `&mut str` keeps the bytes UTF-8 |
| `SString::freeze` | `SString` by value | `SVec::freeze` |
| `SString::freeze_into` | `SString` by value, `&mut Frame<'g>` | `SVec::freeze_into` |
| `sformat!` (macro) | `&mut Frame<'f>` | `SString::new_in`, `fmt::Write::write_fmt`, `SString::freeze`; no code of its own touches the stack |

### `SMap`

| Function | Takes | Preserves I, T, E, B, M because |
|---|---|---|
| `SMap::new_in` | `&'v mut Frame<'f>` | `with_hasher_in` with a fresh `RandomState` |
| `SMap::with_capacity_in` | `&'v mut Frame<'f>` | `with_capacity_and_hasher_in` with a fresh `RandomState` |
| `SMap::with_hasher_in` | `&'v mut Frame<'f>` | no block; end is `frame.end`; holds the frame for `'v` |
| `SMap::with_capacity_and_hasher_in` | `&'v mut Frame<'f>` | `with_hasher_in`, then `reserve` |
| `SMap::len` | `&SMap` | reads `len` |
| `SMap::is_empty` | `&SMap` | reads `len` |
| `SMap::capacity` | `&SMap` | reads `slots`; at most `u32::MAX - 1` |
| `SMap::hasher` | `&SMap` | a shared reference to the hasher |
| `SMap::reserve` | `&mut SMap` | `try_reserve`, panicking on error |
| `SMap::try_reserve` | `&mut SMap` | grows only through `try_grow_to`: resets to its own end, then bumps (no block yet) or extends at `top` (B, T); entries never move; the index is zeroed and rebuilt from stored hashes by safe code (M); no user code; on error no field changes |
| `SMap::get_index` | `&SMap` | bounds-checked read through `parts` |
| `SMap::get_index_mut` | `&mut SMap` | bounds-checked; the key stays shared, so the index stays right |
| `SMap::swap_remove_index` | `&mut SMap` | edits slots with bounds-checked indexing, swaps two entries, then `pop_bucket` lowers `len` before reading the last entry out; never touches the stack; no user code |
| `SMap::shift_remove_index` | `&mut SMap` | as `swap_remove_index`, with a rotation of `[index, len)` |
| `SMap::iter` | `&SMap` | slice iterator over `parts`, bounded by the borrow of the map |
| `SMap::iter_mut` | `&mut SMap` | slice iterator over `parts_mut`; keys stay shared |
| `SMap::keys` | `&SMap` | `iter` |
| `SMap::values` | `&SMap` | `iter` |
| `SMap::values_mut` | `&mut SMap` | `iter_mut` |
| `SMap::clear` | `&mut SMap` | `len := 0` and the index emptied, then the entries dropped back to front (H3); keeps the block |
| `SMap::above` | `&mut SMap` | as `SVec::above`: resets to the map's end; the new frame starts there and borrows the map |
| `SMap::insert` | `&mut SMap` | hashes and probes first (H3); then replaces a value in place, or `reserve(1)` and writes entry `len < capacity` (M) and records its slot |
| `SMap::get_index_of` | `&SMap` | hashes and probes through shared references (H3(c)) |
| `SMap::get` | `&SMap` | `get_index_of`, then a bounds-checked read |
| `SMap::get_mut` | `&mut SMap` | `get_index_of`, then a bounds-checked write access to the value |
| `SMap::contains_key` | `&SMap` | `get_index_of` |
| `SMap::swap_remove` | `&mut SMap` | `get_index_of`, then `swap_remove_index` |
| `SMap::shift_remove` | `&mut SMap` | `get_index_of`, then `shift_remove_index` |
| `SMap::entry` | `&mut SMap` | hashes and probes before returning (H3); the entry borrows the map mutably, so nothing changes the map while it exists |
| `Entry::key` | `&Entry` | reads the key |
| `Entry::or_insert` | `Entry` by value | `OccupiedEntry::into_mut` or `VacantEntry::insert` |
| `Entry::or_insert_with` | `Entry` by value | runs `f` before anything is reserved (H3), then `VacantEntry::insert` |
| `Entry::or_default` | `Entry` by value | `or_insert_with(V::default)` |
| `OccupiedEntry::key` | `&OccupiedEntry` | bounds-checked read; the index was found while the map has been borrowed since |
| `OccupiedEntry::index` | `&OccupiedEntry` | reads the index |
| `OccupiedEntry::get` | `&OccupiedEntry` | as `key` |
| `OccupiedEntry::get_mut` | `&mut OccupiedEntry` | as `key`, through `parts_mut` |
| `OccupiedEntry::into_mut` | `OccupiedEntry` by value | as `get_mut`, for the map borrow `'a` |
| `VacantEntry::key` | `&VacantEntry` | reads the key |
| `VacantEntry::insert` | `VacantEntry` by value | `push` with the hash computed by `entry`: `reserve(1)`, write entry `len`, record its slot; no user code between the probe and the push |
| `SMap::freeze` | `SMap` by value | compacts in place: the new index (`slots_for(len) <= slots` slots) is written inside the old block right after the entries; lowers `top` to its end and raises `frame.end` to the same offset (E); returns the brand `'f`. With no block: an empty map and nothing touched; with a block but no entries: `reset_to(frame.end)` gives the block back |
| `SMap::freeze_into` | `SMap` by value, `&mut Frame<'g>` | as `SVec::freeze_into`: block from `dst.try_alloc_array`, one bitwise copy of the entries, source `len := 0` so each entry is owned once, index built in the new block; `dst` is on another stack by the chain property |

### `SFrozenMap`

| Function | Takes | Preserves I, T, E, B, M because |
|---|---|---|
| `SFrozenMap::len` | `&SFrozenMap` | reads `len` |
| `SFrozenMap::is_empty` | `&SFrozenMap` | reads `len` |
| `SFrozenMap::hasher` | `&SFrozenMap` | a shared reference to the hasher |
| `SFrozenMap::get_index` | `&SFrozenMap` | bounds-checked read through `parts` |
| `SFrozenMap::get_index_mut` | `&mut SFrozenMap` | bounds-checked; the key stays shared |
| `SFrozenMap::iter` | `&SFrozenMap` | slice iterator over `parts` |
| `SFrozenMap::iter_mut` | `&mut SFrozenMap` | slice iterator over `parts_mut`; keys stay shared |
| `SFrozenMap::keys` | `&SFrozenMap` | `iter` |
| `SFrozenMap::values` | `&SFrozenMap` | `iter` |
| `SFrozenMap::values_mut` | `&mut SFrozenMap` | `iter_mut` |
| `SFrozenMap::get_index_of` | `&SFrozenMap` | hashes and probes through shared references; the user code may use the frame (H3(b)) |
| `SFrozenMap::get` | `&SFrozenMap` | `get_index_of`, then a bounds-checked read |
| `SFrozenMap::get_mut` | `&mut SFrozenMap` | `get_index_of`, then bounds-checked access through `parts_mut` |
| `SFrozenMap::contains_key` | `&SFrozenMap` | `get_index_of` |

### `FrameAlloc` (feature `allocator-api2`)

| Function | Takes | Preserves I, T, E, B because |
|---|---|---|
| `allocate` | `&FrameAlloc` | `try_bump`; see "The allocator adapter" |
| `deallocate` | `&FrameAlloc` | lowers `top` to the block's start only when the block ends at `top` |
| `grow` | `&FrameAlloc` | in place by `try_extend` when the block ends at `top` and is aligned for the new layout; otherwise a fresh block and a copy (row 79) |
| `grow_zeroed` | `&FrameAlloc` | `grow`, then zeroes the new tail (row 80) |
| `shrink` | `&FrameAlloc` | in place when aligned, lowering `top` to the new end if on top; otherwise a fresh block and a copy (row 81) |

### `AllocError`

| Function | Takes | Preserves I, T, E, B because |
|---|---|---|
| `AllocError::kind` | `&AllocError` | plain data |

### Trait impls

| Impl | Preserves the invariants because |
|---|---|
| `Deref`, `DerefMut` for `SBox`, `SSlice`, `SFixedVec`, `SVec` | a reference to the initialised values, bounded by the borrow of the handle |
| `Deref` for `SStr`, `SString` | the same bytes as `str`; they are UTF-8 by construction |
| `Drop` for `SBox`, `SSlice`, `SFixedVec`, `SVec`, `IntoIter` (and `VecIntoIter` through it) | drops the owned values (back to front where there are several); never writes `top` or an `end` |
| `Drop` for `Stack` (through `Region`) | unmaps the region; needs the stack unborrowed, so nothing is reachable |
| `IntoIterator` for `SSlice`; `Iterator`, `DoubleEndedIterator`, `ExactSizeIterator` for `IntoIter` | same block and brand; each value is read out once, tracked by `head` and `tail` |
| `IntoIterator` for `&SSlice`, `&mut SSlice`, `&SFixedVec`, `&mut SFixedVec`, `&SVec`, `&mut SVec` | `slice::iter` / `iter_mut` through `Deref` / `DerefMut` |
| `IntoIterator` for `SFixedVec` | `into_slice`, then `SSlice::into_iter` |
| `IntoIterator` for `SVec`; `Iterator`, `DoubleEndedIterator`, `ExactSizeIterator` for `VecIntoIter` | unsafe row 63; afterwards delegates to `IntoIter`; never writes `top` or an `end`, so the buffer is reclaimed by the next reset of the frame |
| `Extend<T>` for `SVec`; `Extend<char>`, `Extend<&str>`, `fmt::Write` for `SString` | loops over `push` / `push_str` |
| `Drop` for `SMap`, `SFrozenMap` | `len := 0` first, then drops the entries back to front; never writes `top` or an `end` |
| `Extend<(K, V)>` for `SMap` | `try_reserve` of the lower size hint (its error is ignored), then loops over `insert` |
| `Debug` for `SMap`, `SFrozenMap` | `debug_map` over `iter` |
| `IntoIterator` for `&SMap`, `&mut SMap`, `&SFrozenMap`, `&mut SFrozenMap`; `Iterator`, `DoubleEndedIterator`, `ExactSizeIterator` for `map::Iter`, `IterMut`, `Keys`, `Values`, `ValuesMut` | `iter` / `iter_mut`; each wraps a slice iterator over the entries |
| `Default`, `Debug` for `Stack` | `Stack::new`; reads three integers |
| `Debug`, `PartialEq`, `Eq`, `PartialOrd`, `Ord`, `Hash`, `Borrow` (`deref_traits!`); `Display` for `SStr`, `SString`; `Display`, `Error` for `AllocError` | delegate to the `Deref` target through `&self` |

## Every `unsafe` block

`grep -o "unsafe {" crates/ordstack/src/*.rs` finds 81 blocks: 78 in library
code and 3 in the unit tests of `region.rs`. There is one numbered row for
each, in file order, except that `alloc.rs` (feature `allocator-api2`)
comes last as rows 79–81; `scripts/check-unsafe-table.sh`, run by
`scripts/check.sh`, fails when the two counts differ. "Handle contract" means the safety contract of the
constructor that built the handle (`SBox::from_raw`,
`SSlice::from_raw_parts`): the block is allocated and unaliased for the
brand and holds initialised values, or the pointer is dangling and aligned
and the block has zero size. The section "Why frozen blocks stay valid for
their brand" is what discharges it.

There are also 9 `unsafe fn` private to the crate, plus the 4 `unsafe fn`
of the `Allocator` trait that `FrameAlloc` implements (`deallocate`,
`grow`, `grow_zeroed`, `shrink`), whose contracts are the trait's; the 9 private contracts
follow the table. There is one `unsafe impl`, of `allocator_api2::alloc::Allocator` for
`FrameAlloc` (feature `allocator-api2`; justified in "The allocator
adapter"), and no `unsafe trait`.

| # | File | Function | Operation | Justified by |
|---|---|---|---|---|
| 1 | `boxed.rs` | `SBox::into_inner` | `ptr.read()` | handle contract; the box is wrapped in `ManuallyDrop`, so the value is not dropped a second time |
| 2 | `boxed.rs` | `SBox::leak` | `&mut *ptr` for `'f` | handle contract (valid and unaliased for `'f`); the forgotten box was the only owner; `T: 'f` |
| 3 | `boxed.rs` | `Deref::deref` | `ptr.as_ref()` | handle contract |
| 4 | `boxed.rs` | `DerefMut::deref_mut` | `ptr.as_mut()` | handle contract; `&mut self` is unique |
| 5 | `boxed.rs` | `Drop::drop` | `drop_in_place(ptr)` | handle contract; the box is not used again |
| 6 | `frame.rs` | `Frame::alloc_slice_copy` | `copy_nonoverlapping` into the block, then `SSlice::from_raw_parts` | fresh block for `src.len()` values from `try_alloc_array`, disjoint from `src` (I: `src` is reachable, the fresh block was not); `T: Copy`; E for the handle contract |
| 7 | `frame.rs` | `alloc_slice_clone`, `Done::drop` | `drop_back_to_front(ptr, n)` | the loop initialised exactly the first `n` slots |
| 8 | `frame.rs` | `Frame::alloc_slice_clone` | `ptr.add(n).write(clone)` | `n < src.len()`: inside the fresh block, slot not yet initialised |
| 9 | `frame.rs` | `Frame::alloc_slice_clone` | `SSlice::from_raw_parts` | all `src.len()` slots initialised; E |
| 10 | `frame.rs` | `Frame::alloc_str` | `SStr::from_utf8_unchecked` | the bytes were copied from a `str` |
| 11 | `frame.rs` | `Frame::try_alloc` | `ptr.write(value)`, then `SBox::from_raw` | fresh aligned block for one `T` (or dangling and aligned for a zero-size `T`); E |
| 12 | `slice.rs` | `drop_back_to_front`, `Rest::run` | `drop_in_place(ptr.add(left))` | caller's contract (`len` owned initialised values); `left` is lowered before each drop, so each index is visited once even when a destructor panics |
| 13 | `slice.rs` | `SSlice::leak` | `slice::from_raw_parts_mut` for `'f` | handle contract; the slice is forgotten; `T: 'f` |
| 14 | `slice.rs` | `Deref::deref` for `SSlice` | `slice::from_raw_parts` | handle contract |
| 15 | `slice.rs` | `DerefMut::deref_mut` for `SSlice` | `slice::from_raw_parts_mut` | handle contract; `&mut self` is unique |
| 16 | `slice.rs` | `Drop::drop` for `SSlice` | `drop_back_to_front(ptr, len)` | handle contract; `self.len` was set to 0 first |
| 17 | `slice.rs` | `IntoIter::next` | `ptr.add(head - 1).read()` | indices in `[head, tail)` hold values not yet yielded; `head` was advanced, so the slot is not read again |
| 18 | `slice.rs` | `IntoIter::next_back` | `ptr.add(tail).read()` | as above, with `tail` lowered first |
| 19 | `slice.rs` | `Drop::drop` for `IntoIter` | `drop_back_to_front(ptr.add(head), tail - head)` | those are the values not yet yielded; the range was emptied first |
| 20 | `slice.rs` | `SStr::leak` | `str::from_utf8_unchecked_mut` | `SStr` is built only from `str` bytes (`from_utf8_unchecked`'s contract) |
| 21 | `slice.rs` | `Deref::deref` for `SStr` | `str::from_utf8_unchecked` | as above |
| 22 | `string.rs` | `SString::split_above` | `str::from_utf8_unchecked_mut` | the bytes are UTF-8 (every `SString` mutation appends whole `str`s or cuts at a character boundary); `&mut str` allows only edits that keep them so |
| 23 | `string.rs` | `SString::freeze` | `SStr::from_utf8_unchecked` | as above |
| 24 | `string.rs` | `SString::freeze_into` | `SStr::from_utf8_unchecked` | as above; the copy is bitwise |
| 25 | `string.rs` | `Deref::deref` for `SString` | `str::from_utf8_unchecked` | as above |
| 26 | `fixed.rs` | `SFixedVec::try_push` | `ptr.add(len).write(value)` | `len < cap` was just checked: inside the block, slot uninitialised |
| 27 | `fixed.rs` | `SFixedVec::pop` | `ptr.add(len).read()` | the slot was initialised and `len` was lowered first |
| 28 | `fixed.rs` | `SFixedVec::into_slice` | `SSlice::from_raw_parts(ptr, len)` | first `len` values initialised, block allocated for `'f` by `try_alloc_array` (E); `self` is forgotten |
| 29 | `fixed.rs` | `Deref::deref` | `slice::from_raw_parts` | first `len` values initialised; block valid for the brand |
| 30 | `fixed.rs` | `DerefMut::deref_mut` | `slice::from_raw_parts_mut` | as above; `&mut self` is unique |
| 31 | `fixed.rs` | `Drop::drop` | `drop_back_to_front(ptr, len)` | owns `len` initialised values; `self.len` was set to 0 first |
| 32 | `region.rs` | `Region::release_pages` | `base.add(start)`, `os::discard` | `start < len` was checked, so the range is inside the mapping; `start` is a multiple of the page size and the base is page aligned because `mmap` returned it (a misaligned address would make `madvise` fail and discard nothing); that nothing relies on the contents is the callers' contract (H10, `release_above`) |
| 33 | `region.rs` | `Drop::drop` for `Region` | `os::unmap(base, len)` | `len > 0`, and `base`/`len` are what `os::map` returned; `Region` is not `Clone` |
| 34 | `region.rs` | `os::page_size` (Unix) | `libc::sysconf` | no preconditions |
| 35 | `region.rs` | `os::map` (Unix) | `libc::mmap`, anonymous, private, kernel-chosen address | touches no existing memory; failure is checked |
| 36 | `region.rs` | `os::unmap` (Unix) | `libc::munmap` | caller's contract: a live mapping from `map` |
| 37 | `region.rs` | `os::discard` (Unix) | `libc::madvise` | caller's contract: a page-aligned range inside a live mapping whose contents nothing relies on |
| 38 | `region.rs` | `os::map` (Miri) | `std::alloc::alloc` | the layout has non-zero size (`Region::reserve` returns early for 0) |
| 39 | `region.rs` | `os::unmap` (Miri) | `std::alloc::dealloc` | caller's contract; same layout as `map` |
| 40 | `region.rs` | test `reserve_is_writable_end_to_end` | raw writes and a read at offsets `0` and `len - 1` | both inside the region just reserved |
| 41 | `region.rs` | test `released_pages_stay_usable` | raw write at offset 8192 | inside the 64 KiB region |
| 42 | `region.rs` | test `released_pages_stay_usable` | raw write at offset 8192 after `release_pages` | the page is still mapped after being discarded |
| 43 | `stack.rs` | `Stack::ptr_at` | `base.add(off)` | `off <= len` is asserted on the line above in every build; the region is at most `isize::MAX` bytes |
| 44 | `vec.rs` | `SVec::try_push` | `ptr.add(len).write(value)` | `len < cap` (growth succeeded or was not needed): inside the buffer (B), slot holds no value; dangling and aligned for a zero-size `T` |
| 45 | `vec.rs` | `SVec::pop` | `ptr.add(len).read()` | B; `len` was lowered first |
| 46 | `vec.rs` | `SVec::extend_from_slice` | `copy_nonoverlapping` into spare capacity | `reserve` made room for `src.len()` more; spare capacity is reachable only through `&mut self`, so `src` cannot overlap it; `T: Copy` |
| 47 | `vec.rs` | `SVec::insert` | shift the tail up by one, write the slot | `index <= len` asserted; `reserve(1)` gave capacity `len + 1`; B |
| 48 | `vec.rs` | `SVec::remove` | read the slot, shift the tail down | `index < len` asserted; B; `len` is lowered before returning |
| 49 | `vec.rs` | `SVec::swap_remove` | read the slot, move the last element into it | `index < len` asserted; B |
| 50 | `vec.rs` | `SVec::truncate` | `drop_back_to_front(ptr.add(len), n)` | those `n` values were initialised (B) and `self.len` was lowered first |
| 51 | `vec.rs` | `retain`, `Gap::drop` | `ptr::copy` of the unvisited tail down over the gap | `[read, orig)` initialised, `[write, read)` holds no values, `write <= read` |
| 52 | `vec.rs` | `SVec::retain` | `base.add(read)` | `read < orig <= cap`: inside the buffer |
| 53 | `vec.rs` | `SVec::retain` | `&*cur` for the predicate | the slot holds an unvisited value; the reference ends before the value is moved or dropped |
| 54 | `vec.rs` | `SVec::retain` | `ptr::copy(cur, base.add(write), 1)` | slot `write` holds no value, or is `cur` itself |
| 55 | `vec.rs` | `SVec::retain` | `drop_in_place(cur)` | the value is initialised; `read` is already past it, so the guard will not touch it |
| 56 | `vec.rs` | `SVec::split_above` | `slice::from_raw_parts_mut(ptr, len)` beside a new frame | B; the frame starts at the vector's end and hands out memory only at or above it, so it never aliases the elements; both results borrow the vector |
| 57 | `vec.rs` | `SVec::into_parts` | `ptr::read(&this.frame)` | `this` is in `ManuallyDrop` and never used again, so the `&mut Frame` is moved, not duplicated |
| 58 | `vec.rs` | `SVec::freeze` | `SSlice::from_raw_parts(ptr, len)` | B; `frame.end` was just raised past the elements (E); with no buffer the block has zero size and `ptr` is dangling and aligned |
| 59 | `vec.rs` | `SVec::freeze_into` | `copy_nonoverlapping` into `dst`'s block, `len := 0`, `SSlice::from_raw_parts` | fresh block for `len` values from `dst.try_alloc_array`; `self` and `dst` are both usable, so neither borrows the other, and by the chain property (Definitions) they are on different stacks: the block is in another mapping than the source buffer; `len := 0` leaves one owner |
| 60 | `vec.rs` | `Deref::deref` | `slice::from_raw_parts` | B |
| 61 | `vec.rs` | `DerefMut::deref_mut` | `slice::from_raw_parts_mut` | B; `&mut self` is unique |
| 62 | `vec.rs` | `Drop::drop` | `drop_back_to_front(ptr, len)` | B; `self.len` was set to 0 first |
| 63 | `vec.rs` | `IntoIterator::into_iter` (`SVec` → `VecIntoIter`) | `SSlice::from_raw_parts(ptr, len)` with brand `'v` | B for `len` values; the buffer lies at or above `frame.end`, and `VecIntoIter` holds `&'v mut Frame<'f>` for as long as the slice iterator inside it exists, so neither this frame nor any object beneath it can reset (I); the slice iterator is dropped first (field order) |
| 64 | `map.rs` | `SMap::parts` | two `slice::from_raw_parts`: entries, index | M: `len` initialised entries at the start of the block and `slots` initialised slots after the first `usable(slots)` entries; disjoint because `len <= usable(slots)`; with no block both lengths are 0 and `ptr` is dangling and aligned |
| 65 | `map.rs` | `SMap::parts_mut` | two `slice::from_raw_parts_mut` | as row 64; the ranges are disjoint and `&mut self` is unique |
| 66 | `map.rs` | `SMap::try_grow_to` | `write_bytes(0)` over the new index | the block was just extended or bumped to `block_bytes(new_slots)` bytes at `top` (B, T); the index starts after `usable(new_slots) > len` entries, so it overlaps no entry |
| 67 | `map.rs` | `SMap::push` | `ptr.add(len).write(bucket)` | `reserve(1)` gave `len < usable(slots)`: inside the block, holding no entry (M) |
| 68 | `map.rs` | `SMap::pop_bucket` | `ptr.add(len).read()` | M; `len` was lowered first; both callers removed the only slot naming that entry first |
| 69 | `map.rs` | `SMap::clear` | `drop_back_to_front(ptr, len)` | M; `self.len := 0` and the index emptied first |
| 70 | `map.rs` | `Drop::drop` for `SMap` | `drop_back_to_front(ptr, len)` | M; `self.len := 0` first |
| 71 | `map.rs` | `SMap::into_parts` | `ptr::read` of the frame reference and the hasher | `this` is in `ManuallyDrop` and never used again, so both are moved, not duplicated |
| 72 | `map.rs` | `SMap::freeze` | `write_bytes(0)` over the compact index | `len <= usable(slots)` and `slots_for(len) <= slots`, so `[len * E, len * E + new_slots * 4)` lies inside the old block, past the entries |
| 73 | `map.rs` | `SMap::freeze` | `SFrozenMap::from_raw_parts` | M for the entries; the index was zeroed (row 72) and is then filled by safe code; `frame.end` was just raised past the block (E) |
| 74 | `map.rs` | `SMap::freeze_into` | `copy_nonoverlapping` of the entries, `len := 0`, `write_bytes(0)` over the index | fresh block of `units` entries from `dst.try_alloc_array`, and `units * E >= len * E + new_slots * 4`; another stack than the source block, as row 59; `len := 0` leaves one owner |
| 75 | `map.rs` | `SMap::freeze_into` | `SFrozenMap::from_raw_parts` | the block was allocated for `'g` by `dst` (E); entries copied, index zeroed |
| 76 | `map.rs` | `SFrozenMap::parts` | two `slice::from_raw_parts` | `from_raw_parts`'s contract: `len` entries then `slots` slots, disjoint, valid for the brand; empty: both lengths 0, `ptr` dangling and aligned |
| 77 | `map.rs` | `SFrozenMap::parts_mut` | two `slice::from_raw_parts_mut` | as row 76; `&mut self` is unique |
| 78 | `map.rs` | `Drop::drop` for `SFrozenMap` | `drop_back_to_front(ptr, len)` | as row 76; `self.len := 0` first |
| 79 | `alloc.rs` | `FrameAlloc::grow` | `copy_nonoverlapping(ptr, fresh, old.size())` | the caller's contract: `ptr` is a live block of `old.size()` bytes from this adapter; `fresh` was just bumped above every live block, so they are disjoint |
| 80 | `alloc.rs` | `FrameAlloc::grow_zeroed` | `grow`, then `write_bytes` on `[old.size(), new.size())` | the caller's contract is `grow`'s; the result holds `new.size()` bytes |
| 81 | `alloc.rs` | `FrameAlloc::shrink` | `copy_nonoverlapping(ptr, fresh, new.size())` | as row 79, copying `new.size() <= old.size()` bytes |

The `unsafe fn` contracts:

| File | Function | Caller must ensure |
|---|---|---|
| `boxed.rs` | `SBox::from_raw` | `ptr` points to an initialised `T` in a block allocated and unaliased for `'f`, or is dangling and aligned for a zero-size `T` |
| `slice.rs` | `drop_back_to_front` | `ptr` points to `len` initialised values the caller owns and will not touch again |
| `slice.rs` | `SSlice::from_raw_parts` | as `SBox::from_raw`, for `len` values |
| `slice.rs` | `SStr::from_utf8_unchecked` | the bytes are valid UTF-8 |
| `region.rs` | `os::unmap` (Unix and Miri) | `base`/`len` is a live mapping returned by `map` |
| `region.rs` | `os::discard` (Unix and Miri) | the page-aligned range lies inside a live mapping and nothing relies on its contents (a no-op under Miri) |
| `map.rs` | `SFrozenMap::from_raw_parts` | `ptr` points to `len` initialised entries followed by `slots` initialised index slots, in a block that stays allocated and unaliased for `'f` |

## Verification

`scripts/check.sh` runs every job below and stops at the first failure. It
is meant for a large machine.

- `cargo fmt --check`, and `cargo clippy --all-targets --all-features -- -D warnings` with
  `clippy::undocumented_unsafe_blocks` denied.
- The test suite on stable, debug and release, with the default features
  and with `--all-features`, including the doctests of the crate
  documentation and of `README.md`.
- **Compile-fail tests.** 24 cases in `tests/ui/fail`, one per rejection the
  guarantee depends on, each with a compiling twin in `tests/ui/pass` that
  differs only in the offending line. The twins are generated by
  `scripts/gen-ui-pass.sh`; `check.sh` regenerates them and fails if they
  differ from what is committed. Compiler messages are pinned to toolchain
  1.85, so the suite runs only with `ORDSTACK_UI=1` on that toolchain, with
  `--all-features`; a
  plain `cargo test` skips it.
- **Miri** runs the tests, with `--all-features`, under Stacked Borrows
  and again under Tree Borrows.
- **AddressSanitizer** runs the integration and unit tests (not the
  doctests), with `--all-features`, with a sanitised standard library.
- **Shadow-model property test.** `tests/common/model.rs` is an interpreter
  whose 25 byte values (`byte % 25`) select 17 distinct operations on a
  vector of drop-tracked values (nine of the values are push): push, pop,
  truncate, insert, remove, swap_remove, retain, clear, extend, reserve,
  shrink_to_fit, descend through `split_above` (to depth 6), leave the level
  (at depth 0: drop the vector and start a fresh one), freeze,
  `freeze_into` through a second stack, sub-frame scopes with
  `reset_above`, and a map scope: an `SMap<u8, Tracked>` built above the
  vector through `split_above`, driven by its own sub-script (insert,
  `swap_remove`, `shift_remove`, `entry().or_insert_with`, a box allocated
  above the map) with every step checked against a `Vec` model of the
  insertion order and a lookup of every key, and ended by dropping, `freeze`
  (then a box beside the frozen map), or `freeze_into` the scratch stack. With the `allocator-api2` feature,
  op 23 opens an allocator scope instead of pushing: two `allocator_api2`
  vectors on one `FrameAlloc` above the vector, driven by push, pop,
  reserve, shrink_to_fit and clear on either vector, compared with `Vec`
  models after every step and dropped in either order. After every operation the contents are compared with a
  `Vec` model, frozen blocks are re-read after later allocations, and at the
  end every value must have been dropped exactly once in the expected
  order. `tests/model.rs`
  runs it on 2000 random scripts of 0 to 599 bytes (16 scripts under Miri) and on
  fixed scripts.
- **Fuzz target.** `fuzz/fuzz_targets/model.rs` drives the same interpreter
  (the fuzz crate enables `allocator-api2` by default)
  from bytes. The interpreter reads at most 4096 bytes of input and the
  fuzzer is run with `-max_len=4096`.
- **Panic-path tests** in `tests/panics.rs` for the rows of H3.
- **Debug shadow** (H9) is active in every job above that builds with
  `debug_assertions` (not the release test run).

## Known limits of this argument

- It is informal. No model of Rust's borrow checker is used beyond "a `&mut`
  borrow is exclusive for its lifetime" and the drop-check rule in H2.
- Miri runs on a heap-backed region, not the real mapping, and its `madvise`
  path is a no-op.
- AddressSanitizer sees the region as one valid mapping, so it cannot detect
  a use of reclaimed memory inside the region. That is what the debug shadow
  and the model's content checks are for.
- The property test and fuzzer explore one element type, one map type and one thread.
  `SFixedVec`, `SString`, `sformat!`, `Frame::alloc_slice_from_iter`, by-value
  iteration of an `SVec` (`VecIntoIter`), `SMap::clear`, `Extend` for `SMap`, `SMap` growth failure,
  allocation failure, `Stack::reset`
  and `Stack::trim` are outside the interpreter and are covered only by the
  hand-written tests.
- The compile-fail tests show that particular programs are rejected. They
  are examples of I, not a proof of it.
- Address alignment is enforced by an `assert_eq!` in `Region::reserve`: a platform whose mappings are not 4096-aligned cannot create a stack (H6).
- `Stack::offset_of` and `Stack::release_above` check their contracts only
  in debug builds (H10).
- Only Linux has been run through `scripts/check.sh`. The macOS code path
  (`MADV_FREE`) compiles from the same source but its test run is a CI job
  that has not executed yet.
