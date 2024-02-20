#![no_std]
#![forbid(
    unsafe_op_in_unsafe_fn,
    clippy::missing_safety_doc,
    clippy::undocumented_unsafe_blocks,
    clippy::suspicious_doc_comments,
    clippy::missing_const_for_fn,
    clippy::suspicious,
    clippy::branches_sharing_code,
    clippy::bad_bit_mask,
    clippy::std_instead_of_core,
    clippy::alloc_instead_of_core,
    clippy::std_instead_of_alloc
)]
#![cfg_attr(
    not(test),
    forbid(clippy::print_stderr, clippy::print_stdout, clippy::todo)
)]
#![deny(clippy::perf, clippy::arithmetic_side_effects, unused_unsafe)]

#[cfg(feature = "alloc")]
extern crate alloc;

#[macro_use]
#[cfg(feature = "std")]
extern crate std;

pub mod interface;

mod ext;
pub mod strategy;

pub mod delay;
#[cfg(feature = "alloc")]
pub mod op;
pub mod raw;
