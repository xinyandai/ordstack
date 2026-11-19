//! Stack-allocated containers whose stack discipline is proved by the borrow
//! checker.
//!
//! # The guarantee
//!
//! A [`Stack`] is one contiguous address range with a bump pointer. For every
//! stack, in safe code:
//!
//! 1. only the top block is ever grown, and it grows in place;
//! 2. memory is reclaimed only when no live value can reach it.
//!
//! So the allocator keeps no per-block header, never copies a block to grow
//! it, has no slow path for a block that is not on top, and never abandons a
//! buffer because it had to grow from beneath another block. Code that would
//! break the discipline does not compile. (Internally, growth still asserts
//! that the block it extends ends at the bump pointer. That is a check for
//! bugs in this crate, in every build; no safe program can make it fail.)
//! The optional `FrameAlloc` adapter is the exception: it copies blocks
//! that are not on top, see *Collections generic over an allocator*.
//!
//! This is a typed region arena with a growable top. Frozen blocks are not
//! freed one by one, and the crate makes no speed or memory claim against a
//! general-purpose allocator.
//!
//! # Example
//!
//! ```
//! use ordstack::{SVec, Stack};
//!
//! let mut st = Stack::new();
//! let mut f = st.frame();
//!
//! let mut tokens: SVec<&str> = SVec::new_in(&mut f);
//! tokens.extend("let x = 1".split(' ')); // grows in place: it is the top block
//!
//! {
//!     // From here `tokens` is frozen: readable through `toks`, not growable.
//!     let (toks, mut above) = tokens.split_above();
//!     let mut lens: SVec<usize> = SVec::new_in(&mut above);
//!     lens.extend(toks.iter().map(|t| t.len()));
//!     assert_eq!(&*lens, [3, 1, 1, 1]);
//!     // tokens.push("oops"); // would not compile here
//! }
//!
//! tokens.push(";"); // top again; growing it reclaims the space `lens` used
//! assert_eq!(tokens.len(), 5);
//! ```
//!
//! # Freeze and descend
//!
//! New space above an object is obtained only by mutably borrowing it:
//! [`Stack::frame`], [`Frame::sub`], [`SVec::above`], [`SVec::split_above`]
//! (and the same two methods on [`SString`]). While the returned [`Frame`],
//! or anything allocated through it, is alive, the object beneath cannot grow
//! or allocate. Holding `&mut` to a growable container is the proof that it
//! is the top block.
//!
//! ```compile_fail,E0499
//! use ordstack::{SVec, Stack};
//! let mut st = Stack::new();
//! let mut f = st.frame();
//! let mut lower: SVec<u32> = SVec::new_in(&mut f);
//! let mut above = lower.above();
//! let mut upper: SVec<u32> = SVec::new_in(&mut above);
//! lower.push(1); // error: `lower` is mutably borrowed by `above`
//! upper.push(2);
//! ```
//!
//! # Frozen blocks
//!
//! [`Frame::alloc`] and friends return [`SBox`], [`SSlice`] and [`SStr`]:
//! owning handles to blocks that never move or grow. They are branded with
//! the frame's lifetime, so several can sit side by side and outlive the
//! `&mut` borrow that allocated them. [`SVec::freeze`] turns a growable
//! vector into a frozen slice without copying. [`sformat!`] formats into an
//! [`SStr`] the way `format!` formats into a `String`.
//!
//! ```
//! use ordstack::{SString, Stack};
//!
//! let mut st = Stack::new();
//! let mut f = st.frame();
//! let mut s = SString::new_in(&mut f);
//! s.push_str("hello");
//! let hello = s.freeze(); // the frame is free again
//! let n = f.alloc(hello.len()); // sits beside `hello`
//! assert_eq!((hello.as_str(), *n), ("hello", 5));
//! ```
//!
//! # Maps
//!
//! [`SMap`] is a hash map that grows on a frame the way an [`SVec`] does: it
//! holds the frame, keeps its entries in insertion order in one block, and
//! grows that block in place. Iteration follows insertion order, so output
//! never depends on the hasher. [`SMap::freeze`] turns it into an
//! [`SFrozenMap`] where it stands, compacting the index and giving the rest
//! of the block back, so the frame is free again; [`SMap::freeze_into`]
//! moves it to a frame of another stack.
//!
//! ```
//! use ordstack::{SFrozenMap, SMap, Stack};
//!
//! let mut st = Stack::with_reserve(1 << 16);
//! let mut f = st.frame();
//! let mut m: SMap<&str, u32> = SMap::new_in(&mut f);
//! for w in "the cat saw the dog".split(' ') {
//!     *m.entry(w).or_default() += 1;
//! }
//! let counts: SFrozenMap<&str, u32> = m.freeze();
//! let total = f.alloc(counts.values().sum::<u32>()); // the frame is free again
//! assert_eq!(counts.get("the"), Some(&2));
//! assert_eq!(counts.keys().copied().collect::<Vec<_>>(), ["the", "cat", "saw", "dog"]);
//! assert_eq!(*total, 5);
//! ```
//!
//! # Pools
//!
//! [`SPool`] is a growable, index-addressed pool whose free slots are reused.
//! [`SFixedPool`] has a fixed capacity and does not hold its frame, so values
//! owned by the pool and their children can share the stack. `freeze` turns a
//! growable pool into a fixed pool without changing existing indices.
//!
//! ```
//! use ordstack::{Slot, SPool, Stack};
//!
//! let mut st = Stack::with_reserve(1 << 16);
//! let mut f = st.frame();
//! let mut pool = SPool::<Slot<&str>>::with_capacity_in(2, &mut f);
//! let first = pool.alloc("first");
//! let second = pool.alloc("second");
//! pool.free(first);
//! let reused = pool.alloc("replacement");
//! assert_eq!(reused, first);
//! assert_eq!(pool.get(second), Some(&"second"));
//! let frozen = pool.freeze();
//! let more = f.alloc(1u32);
//! assert_eq!((*more, frozen.get(reused)), (1, Some(&"replacement")));
//! ```
//!
//! # Collections generic over an allocator
//!
//! With the `allocator-api2` feature, `Frame::allocator` returns a
//! `FrameAlloc`, an allocator for collections generic over
//! `allocator_api2::alloc::Allocator`: `allocator_api2::vec::Vec`,
//! `hashbrown`'s maps and sets, and others. Any number of them can grow on
//! one frame at once through `&FrameAlloc`; the one on top grows in place,
//! the others copy when they grow. Everything allocated through the adapter
//! is reclaimed when the frame is next used after it.
//!
//! Choose `SVec`, `SString` and `SMap` when the result should outlive the
//! scope that builds it (`freeze` gives frame-lifetime handles in place,
//! `freeze_into` copies to another stack), when insertion order matters
//! (`SMap`), or when no space may be wasted. Choose `SPool` when stable,
//! reusable indices matter and `SFixedPool` when those slots and values that
//! allocate on the same frame must coexist. Choose the adapter to reuse
//! code written for `Vec<T, A>` or `HashMap<K, V, S, A>`, or to grow
//! several collections on one frame at once; growth that is not at the top
//! copies and leaves dead space until the adapter's memory is reclaimed,
//! and nothing allocated through it outlives the adapter. As with `std`, a
//! failed allocation in an infallible method such as `push` aborts; use the
//! collection's `try_reserve` to handle it.
//!
//! # Reclamation is lazy
//!
//! Nothing is reclaimed when a handle is dropped; destructors only drop
//! values. (Dropping the [`Stack`] itself unmaps its whole range, which the
//! borrow checker allows only once nothing borrows it.) Space above an
//! object is reclaimed the next time that object needs the top: when a frame
//! allocates or hands out a sub-frame, and when a vector grows, shrinks,
//! freezes or hands out a frame. Operations that stay inside existing
//! capacity leave it alone. To reclaim sooner call [`Frame::reset_above`] or
//! [`Stack::reset`]; to also return pages to the operating system call
//! [`Frame::trim_above`] or [`Stack::trim`]. Leaking a handle is safe: its
//! destructor does not run, and its space is held until the object beneath is
//! next used in one of those ways.
//!
//! # Failure
//!
//! Allocating methods panic, with a message starting `ordstack: `, when the
//! reservation is used up, a size overflows, or a value needs alignment
//! above 4096. Creating a stack panics when the requested reservation is too
//! large or the operating system refuses the mapping.
//! [`Stack::try_with_reserve`], [`Frame::try_alloc`],
//! [`SVec::try_push`], [`SVec::try_reserve`], [`SString::try_reserve`] and
//! [`SMap::try_reserve`] return an [`AllocError`] instead.
//!
//! A failed allocation changes no object: lengths, capacities, pointers and
//! frame ends are as before, and no stored value is lost.
//! [`SVec::try_push`] hands the rejected value back; [`Frame::try_alloc`]
//! drops the value it was given. A failed allocation may still lower
//! [`Stack::extent`], because the lazy reclamation described above runs
//! before the step that can fail.
//!
//! ```
//! use ordstack::{AllocErrorKind, SVec, Stack};
//!
//! let mut st = Stack::with_reserve(4096);
//! let room = st.capacity();
//! let mut f = st.frame();
//! let mut v: SVec<u8> = SVec::with_capacity_in(8, &mut f);
//! v.push(1);
//! let err = v.try_reserve(room).unwrap_err();
//! assert_eq!(err.kind(), AllocErrorKind::Exhausted);
//! assert_eq!((v.len(), v.capacity()), (1, 8));
//! ```
//!
//! Zero-size values take no space and never reach the allocator, so they do
//! not fail for lack of room or for alignment. (Through the optional
//! `FrameAlloc` adapter they do reach it.)
//!
//! # Patterns
//!
//! **Several stacks.** Two containers that grow in alternation need two
//! stacks. A common split is one stack for results and one for scratch:
//!
//! ```
//! use ordstack::{Frame, SSlice, SVec, Stack};
//!
//! /// Squares of the even numbers, built on `scratch`, returned on `out`.
//! fn even_squares<'o>(
//!     xs: &[u32],
//!     out: &mut Frame<'o>,
//!     scratch: &mut Frame<'_>,
//! ) -> SSlice<'o, u32> {
//!     let mut tmp: SVec<u32> = SVec::new_in(scratch);
//!     tmp.extend(xs.iter().filter(|x| *x % 2 == 0).map(|x| x * x));
//!     tmp.freeze_into(out) // one copy; the scratch space is reclaimed later
//! }
//!
//! let (mut a, mut b) = (Stack::new(), Stack::new());
//! let (mut out, mut scratch) = (a.frame(), b.frame());
//! let r = even_squares(&[1, 2, 3, 4], &mut out, &mut scratch);
//! assert_eq!(&*r, [4, 16]);
//! ```
//!
//! [`SVec::freeze_into`] needs a destination frame that the vector does not
//! itself hold, which in practice means a frame of another stack.
//!
//! **Children on the same stack.** A growable container holds its frame, so
//! its elements cannot allocate from it. When the number of children is known
//! first, use [`SFixedVec`], which does not hold the frame:
//!
//! ```
//! use ordstack::{Frame, SFixedVec, SSlice, Stack};
//!
//! struct Node<'f> {
//!     depth: u32,
//!     kids: SSlice<'f, Node<'f>>,
//! }
//!
//! fn build<'f>(depth: u32, f: &mut Frame<'f>) -> Node<'f> {
//!     let n = if depth == 0 { 0 } else { 2 };
//!     let mut kids = SFixedVec::with_capacity_in(n, f);
//!     for _ in 0..n {
//!         kids.push(build(depth - 1, f)); // the children allocate from `f` too
//!     }
//!     Node { depth, kids: kids.into_slice() }
//! }
//!
//! let mut st = Stack::new();
//! let mut f = st.frame();
//! let root = build(2, &mut f);
//! assert_eq!(root.kids.len(), 2);
//! assert_eq!(root.kids[1].kids[0].depth, 0);
//! ```
//!
//! **Phases.** A program that builds data, consumes it, and builds
//! something else can reuse one stack for each phase. Consuming a vector by
//! value keeps its frame borrowed, so its buffer is reclaimed afterwards;
//! blocks that were frozen stay until their frame's parent is reused, and
//! [`Stack::reset`] reclaims everything at once ([`Stack::trim`] also
//! returns the pages to the operating system).
//!
//! ```
//! use ordstack::{SVec, Stack};
//!
//! let mut st = Stack::with_reserve(1 << 20);
//! let total: u64 = {
//!     let mut f = st.frame();
//!     let mut parsed: SVec<u64> = SVec::new_in(&mut f);
//!     parsed.extend(0..1000);
//!     parsed.into_iter().sum() // the buffer is reclaimable from here
//! };
//! assert!(st.high_water() >= 8000);
//! st.reset(); // next phase starts from an empty stack
//! let mut f = st.frame();
//! let out = f.alloc(total);
//! assert_eq!(*out, 499_500);
//! assert_eq!(f.stack().extent(), 8);
//! ```
//!
//! **Choosing a reservation.** [`Stack::new`] reserves
//! [`DEFAULT_RESERVE`] bytes of address space (4 GiB on 64-bit targets).
//! Pages are committed only when touched, so on 64-bit Linux a large
//! reservation costs nothing until it is used. Under an address-space
//! limit (`ulimit -v`), with several stacks, or under Miri (where a
//! reservation is a real heap allocation), use [`Stack::with_reserve`] with
//! a size close to the expected [`high_water`](Stack::high_water):
//!
//! ```
//! use ordstack::Stack;
//!
//! // Small sizes so the example also runs under Miri.
//! let mut results = Stack::with_reserve(1 << 20);
//! let mut scratch = Stack::with_reserve(1 << 16);
//! let (mut out, mut tmp) = (results.frame(), scratch.frame());
//! let _a = out.alloc(1u32);
//! let _b = tmp.alloc(2u32);
//! assert!(out.stack().capacity() >= 1 << 20);
//! ```
//!
//! # Limits
//!
//! - A struct cannot hold a growable container together with the frame it
//!   grows on.
//! - Two containers that grow in alternation need two stacks.
//! - Lifetime parameters spread through signatures.
//! - Frozen blocks are not freed individually.
//! - No stack, frame, container or handle is `Send` or `Sync`.
//! - Unix only in this release; alignment above 4096 is not supported.
//!
//! # Soundness
//!
//! Safety rests on one invariant: every pointer into the region above an
//! object carries a lifetime that keeps that object mutably borrowed. The
//! argument, the hazards considered and a table of every `unsafe` block are
//! in `SOUNDNESS.md`, which ships with the crate
//! ([source on docs.rs](https://docs.rs/crate/ordstack/latest/source/SOUNDNESS.md)). It is argued and tested, not
//! mechanically proved.

#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

#[cfg(not(any(unix, miri)))]
compile_error!(
    "ordstack 0.1 supports Unix targets only (Linux is tested; macOS is intended to work but is untested)"
);

#[cfg(feature = "allocator-api2")]
mod alloc;
mod boxed;
mod debug;
mod error;
mod fixed;
mod frame;
mod macros;
pub mod map;
mod pool;
mod region;
mod slice;
mod stack;
mod string;
mod vec;

#[cfg(feature = "allocator-api2")]
pub use alloc::FrameAlloc;
pub use boxed::SBox;
pub use error::{AllocError, AllocErrorKind};
pub use fixed::SFixedVec;
pub use frame::Frame;
pub use map::{SFrozenMap, SMap};
pub use pool::{Index, PoolSlot, SFixedPool, SPool, Slot};
pub use slice::{SSlice, SStr};
pub use stack::{DEFAULT_RESERVE, Stack};
pub use string::SString;
pub use vec::SVec;

/// Iterator types.
pub mod iter {
    pub use crate::slice::IntoIter;
    pub use crate::vec::VecIntoIter;
}

/// Runs the README's examples as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
