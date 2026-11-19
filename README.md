# ordstack

Stack-allocated growable containers for Rust, where the borrow checker
proves the stack discipline.

- Only the top block of a stack ever grows, and it grows in place.
- Memory is reclaimed only when nothing can reach it.
- Code that would break either rule does not compile. The crate's own
  containers have no per-block header, no copy-on-grow and no slow path for
  a block that is not on top.

```rust
use ordstack::{SVec, Stack};

let mut st = Stack::new();
let mut f = st.frame();
let mut tokens: SVec<&str> = SVec::new_in(&mut f);
tokens.extend("let x = 1".split(' '));
{
    // `tokens` is frozen from here: readable through `toks`, not growable.
    let (toks, mut above) = tokens.split_above();
    let mut lens: SVec<usize> = SVec::new_in(&mut above);
    lens.extend(toks.iter().map(|t| t.len()));
    assert_eq!(&*lens, [3, 1, 1, 1]);
    // tokens.push("oops"); // would not compile here
}
tokens.push(";"); // top again
assert_eq!(tokens.len(), 5);
```

`SMap` is a hash map that grows on a frame the same way, keeps insertion
order, and freezes into an `SFrozenMap` in place. The crate documentation's
*Patterns* section shows several stacks, children on the same stack,
phases, and how to choose a reservation.

`SPool` is a growable, index-addressed pool that reuses free slots. Its
`freeze` operation creates an `SFixedPool` with the reserved capacity and
releases the frame. The fixed pool can allocate from the same frame as the
values stored in its slots. Implement `PoolSlot` to choose a compact custom
slot and index representation, including generational indices that reject
use-after-free after a slot is reused.

With the `allocator-api2` feature, `Frame::allocator` also lets code written
for `Vec<T, A>` or `hashbrown`'s `HashMap` run on a frame, several
collections at once. Unlike the crate's own containers, a collection on the
adapter that grows while it is not on top is copied, and its old block is
held until the adapter ends. The crate documentation's *Collections generic
over an allocator* section says when to use it and when to use `SVec`/`SMap`.

## Several containers on one frame

A growable container (`SVec`, `SString`, `SMap`, `SPool`) holds `&mut` to
its frame, so a frame has at most one at a time. That is what lets it grow
in place: it is always on top. Two at once does not compile:

```rust,compile_fail,E0499
use ordstack::{SMap, SVec, Stack};

let mut st = Stack::new();
let mut f = st.frame();
let mut tokens: SVec<&str> = SVec::new_in(&mut f);
let mut counts: SMap<&str, u32> = SMap::new_in(&mut f); // `f` borrowed twice
tokens.push("a");
counts.insert("a", 1);
```

Frozen values (`SBox`, `SSlice`, `SStr`, `SFrozenMap`, `SFixedPool`) do not
hold the frame, so any number of them can sit on it. Four ways to use more
than one container:

**One after another.** `freeze` ends a container and gives the frame back;
what it built stays readable.

```rust
use ordstack::{SMap, SVec, Stack};

let mut st = Stack::new();
let mut f = st.frame();
let mut tokens: SVec<&str> = SVec::new_in(&mut f);
tokens.extend(["a", "b", "a"]);
let tokens = tokens.freeze(); // an `SSlice`; `f` is free again
let mut counts: SMap<&str, u32> = SMap::new_in(&mut f);
for t in tokens.iter() {
    *counts.entry(*t).or_default() += 1;
}
assert_eq!(counts.get("a"), Some(&2));
```

**On top of a live container.** `above()` gives a frame above it, for
something shorter-lived; the container can grow again once that is gone.

```rust
use ordstack::{SMap, SVec, Stack};

let mut st = Stack::new();
let mut f = st.frame();
let mut tokens: SVec<&str> = SVec::new_in(&mut f);
tokens.push("a");
{
    let mut top = tokens.above();
    let mut seen: SMap<&str, ()> = SMap::new_in(&mut top);
    seen.insert("x", ());
} // `seen` is gone here; its memory is reclaimed when `tokens` next needs the top
tokens.push("b");
assert_eq!(tokens.len(), 2);
```

**Two stacks.** When two containers must grow during the same phase, give
them separate stacks, and copy the result across once, at exact size, with
`freeze_into`.

```rust
use ordstack::{SVec, Stack};

let mut keep = Stack::new();
let mut scratch = Stack::with_reserve(1 << 20); // 1 MiB of scratch space
let mut out = keep.frame();
let mut work = scratch.frame();
let mut words: SVec<&str> = SVec::new_in(&mut work);
words.extend(["pear", "fig", "apple"]);
words.sort_unstable();
let words = words.freeze_into(&mut out); // lives on `keep`; `work` is free
assert_eq!(&*words, ["apple", "fig", "pear"]);
```

**A sub-frame.** `sub()` opens a child frame for temporary work. When it
ends, the parent takes its memory back the next time it allocates, and
values already on the parent stay.

```rust
use ordstack::{SVec, Stack};

let mut st = Stack::new();
let mut f = st.frame();
let name = f.alloc_str("kept");
{
    let mut tmp = f.sub();
    let mut v: SVec<u32> = SVec::new_in(&mut tmp);
    v.extend(0..100);
    assert_eq!(v.iter().sum::<u32>(), 4950);
}
assert_eq!(&*name, "kept");
```

## What it is and is not

`ordstack` is a typed region arena with a growable top. Frozen blocks are
reclaimed in bulk, not one by one, and reclamation is lazy: dropping a handle
runs destructors but returns no memory. It is not an `Allocator` for `std`
collections, not a global allocator, and not thread-safe. It makes no speed
or memory claim against general-purpose allocators.

The crate documentation teaches the model: the guarantee, freeze and descend,
frozen blocks, lazy reclamation, failure, using several stacks, and the
limits.

## How it differs from other bump allocators

What happens when a container has to grow while a later allocation on the
same stack is still alive:

| Crate | Behaviour |
|---|---|
| `ordstack` | The program does not compile. Once the later allocation is dead, the container grows in place, reclaiming the space above it. |
| `bumpalo` | Allowed. Growing an allocation that is not the most recent one copies the contents to a fresh allocation; the old bytes are not reused until the arena is reset. |
| `bump-scope` | Allowed. Growing an allocation that is not the most recent one needs a new allocation, and the old block becomes wasted space. Separately, an allocator that has been claimed fails at run time if it is asked to allocate or grow. |
| `dyn-stack` | Does not arise for its own arrays, which are allocated with a length given up front. |

The other rows describe those crates as their source and documentation
stood at the time of writing; check their current documentation before
relying on them. Those crates are more flexible and far more mature. Use `ordstack` when you want the discipline checked at compile
time.

## Status

0.1, experimental. Unix only; developed and tested on
Linux, and intended to work on macOS. Safety is argued in
[SOUNDNESS.md](https://docs.rs/crate/ordstack/latest/source/SOUNDNESS.md) and tested with compile-fail tests, Miri under
Stacked Borrows and Tree Borrows, AddressSanitizer, a shadow-model property
test and a fuzz target (the repository's `scripts/check.sh` runs all of
them); it is not
mechanically proved. Expect breaking changes before 1.0.

Minimum Rust version: 1.85.

## License

MIT or Apache-2.0, at your option. See [LICENSE-MIT](https://docs.rs/crate/ordstack/latest/source/LICENSE-MIT) and
[LICENSE-APACHE](https://docs.rs/crate/ordstack/latest/source/LICENSE-APACHE).
