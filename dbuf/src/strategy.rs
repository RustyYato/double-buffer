#[cfg(feature = "std")]
#[cfg(feature = "triomphe")]
pub mod flashmap;

#[cfg(feature = "alloc")]
pub mod hazad_flash;

mod atomic;

pub mod simple;
pub mod simple_async;

pub mod park_token;
