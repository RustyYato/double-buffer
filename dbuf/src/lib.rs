#![no_std]

#[cfg(feature = "alloc")]
extern crate alloc;

#[macro_use]
#[cfg(feature = "std")]
extern crate std;

pub mod interface;

mod ext;
pub mod strategy;

pub mod raw;
