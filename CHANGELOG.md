# Changelog

## Unreleased

- `SPool` and `SFixedPool`: growable and fixed-capacity pools with reusable
  `PoolSlot::Index` values, in-place freeze, and custom `PoolSlot` encodings
  that can reject use-after-free with generations

## 0.1.0 (2026-10-15)

First release.

- `Stack`, `Frame`
- Growable `SVec`, `SString`
- Frozen `SBox`, `SSlice`, `SStr`; fixed-capacity `SFixedVec`
- `AllocError`, `AllocErrorKind`; `DEFAULT_RESERVE`
- `SSlice::empty`, `SStr::empty`,
  `SString::capacity`, the `iter` module (to name `IntoIter`), and the
  comparison, hashing and formatting impls on `SFixedVec`
- `Frame::stack`: read a stack's statistics while frames are alive
- `Frame::alloc_slice_from_iter`: collect an iterator into a frozen block in place
- `sformat!`: `format!` into a frozen `SStr` on a frame
- `IntoIterator` for `&SSlice`, `&mut SSlice`, `&SVec`, `&mut SVec`, `&SFixedVec`, `&mut SFixedVec`; by value for `SSlice` and `SFixedVec` (`iter::IntoIter`) and for `SVec` (`iter::VecIntoIter`, which keeps the frame borrowed so the buffer is reclaimed afterwards)
- Crate docs: *Patterns* covers phases and choosing a reservation
- `SMap`: a growable, insertion-ordered hash map on a frame (`map` module); grows in place, `swap_remove`/`shift_remove`, iteration in insertion order whatever the hasher
- `SMap::entry` with `or_insert`, `or_insert_with`, `or_default` (`map::Entry`, `OccupiedEntry`, `VacantEntry`)
- `SFrozenMap`: `SMap::freeze` (in place; the index is compacted and the rest of the block given back) and `SMap::freeze_into` (another stack)
- Feature `allocator-api2`: `Frame::allocator` returns `FrameAlloc`, an `allocator_api2` allocator, so `allocator_api2::vec::Vec`, `hashbrown` and other allocator-generic collections run on a frame
