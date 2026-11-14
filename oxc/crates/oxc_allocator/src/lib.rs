//! # ⚓ Oxc Memory Allocator
//!
//! Oxc uses a bump-based memory arena for faster AST allocations.
//!
//! This crate contains an [`Allocator`] for creating such arenas, as well as ports of data types
//! from `std` adapted to use this arena:
//!
//! * [`Box`]
//! * [`Vec`]
//! * [`String`]
//! * [`HashMap`]
//! * [`HashSet`]
//!
//! See [`Allocator`] docs for information on efficient use of [`Allocator`].
//!
//! ## Features
//!
//! * `serialize` - Enables serialization support for [`Box`] and [`Vec`] with `serde` and `oxc_estree`.
//!
//! * `pool` - Enables `AllocatorPool`.
//!
//! * `bitset` - Enables `BitSet`.
//!
//! * `from_raw_parts` - Adds `Allocator::from_raw_parts` method.
//!   Usage of this feature is not advisable, and it will be removed as soon as we're able to.
//!
//! * `fixed_size` - Makes `AllocatorPool` create large fixed-size allocators, instead of
//!   flexibly-sized ones.
//!   Only supported on 64-bit little-endian platforms at present.
//!   Usage of this feature is not advisable, and it will be removed as soon as we're able to.
//!
//! * `track_allocations` - Count allocations and reallocations.
//!   For internal use only. The APIs provided by this feature are sketchy at best, and possibly
//!   undefined behavior. Do not enable this feature under any circumstances in production code.
//!
//! * `disable_track_allocations` - Disables `track_allocations` feature.
//!   Purpose is to prevent `--all-features` enabling allocation tracking.

#![forbid(unsafe_code)]

mod accessor;
mod address;
mod allocator;
mod boxed;
mod clone_in;
mod convert;
pub mod hash_map;
pub mod hash_set;
pub mod ident_hasher;
mod string_builder;
mod take_in;
mod vec;

pub use accessor::AllocatorAccessor;
pub use address::{Address, GetAddress, UnstableAddress};
pub use allocator::Allocator;
pub use boxed::Box;
pub use clone_in::CloneIn;
pub use convert::{FromIn, IntoIn};
pub use hash_map::HashMap;
pub use hash_set::HashSet;
pub use ident_hasher::IdentBuildHasher;
pub use string_builder::StringBuilder;
pub use take_in::{Dummy, TakeIn};
pub use vec::Vec;
