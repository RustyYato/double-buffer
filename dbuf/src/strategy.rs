#[cfg(feature = "alloc")]
mod hazard;

#[cfg(feature = "std")]
#[cfg(feature = "triomphe")]
pub mod flashmap;

#[cfg(feature = "alloc")]
pub mod hazad_flash;

mod atomic;

pub mod simple;
pub mod simple_async;

#[cfg(feature = "std")]
#[cfg(feature = "triomphe")]
pub mod evmap;
#[cfg(feature = "alloc")]
pub mod hazard_evmap;

pub mod flash_park_token;

pub mod outline_writer;
