//! The raw reader and writer types
//!
//!

use core::{borrow::Borrow, cell::UnsafeCell};

mod reference;

mod reader;
mod writer;

pub use reader::{Reader, ReaderGuard};
pub use writer::Writer;

/// The payload of a double buffer, this holds the two buffers
/// the strategy, and some extra data. (the extra data is for you
/// to use however you want).
pub struct DoubleBufferData<T, S, Extras: ?Sized = ()> {
    buffers: DoubleBufferCell<T>,
    pub strategy: S,
    pub extras: Extras,
}

/// These tests ensure that `DoubleBufferCell` isn't `Send` or `Sync` when that would be unsound
///
/// ```compile_fail,E0412
/// const fn test_send_sync<T: Send, U: Sync>() {
///     let _ = test_send_sync::<DoubleBufferCell<U>, U>;
/// }
/// ```
///
/// ```compile_fail,E0412
/// const fn test_send_sync<T: Send, U: Sync>() {
///     let _ = test_send_sync::<T, DoubleBufferCell<T>>;
/// }
/// ```
///
/// ```compile_fail,E0412
/// const fn test_send_sync<T: Send, U: Sync>() {
///     let _ = test_send_sync::<T, DoubleBufferCell<U>>;
/// }
/// ```
#[repr(transparent)]
struct DoubleBufferCell<T> {
    parts: [UnsafeCell<T>; 2],
}

const _: () = {
    const fn test_send_sync<T: Send, U: Sync>() {
        let _ = test_send_sync::<DoubleBufferCell<T>, DoubleBufferCell<&U>>;
    }
};

// SAFETY:
// This requires `Send`: because the `Writer` can mutate `T` through a shared reference to `DoubleBufferData`
// This requries `Sync`: because the `Reader` can get shared access to `T` through a shared reference to `DoubleBufferData`
unsafe impl<T: Send + Sync> Sync for DoubleBufferCell<T> {}

impl<T> DoubleBufferCell<T> {
    const fn get(&self, swapped: bool) -> (*const T, *mut T) {
        (
            self.parts[(!swapped) as usize].get(),
            self.parts[(swapped) as usize].get(),
        )
    }
}

impl<T, S> DoubleBufferData<T, S> {
    /// Create a new payload with the given buffers and strategy
    #[inline]
    pub const fn new(back: T, front: T, strategy: S) -> Self {
        Self::with_extras(back, front, strategy, ())
    }
}

impl<T, S, Extras> DoubleBufferData<T, S, Extras> {
    /// Create a new payload with the given buffers, strategy, and extra value
    pub const fn with_extras(back: T, front: T, strategy: S, extras: Extras) -> Self {
        Self {
            buffers: DoubleBufferCell {
                parts: [UnsafeCell::new(front), UnsafeCell::new(back)],
            },
            strategy,
            extras,
        }
    }
}

/// This is a type that may be owned or borrowed, like a `Cow`, but this
/// is checked at compile time
///
/// # Safety
///
/// [`Borrow::borrow`] must be a pure function (no side-effects) and must not access any globals
pub unsafe trait MaybeBorrowed<Target: ?Sized>: Borrow<Target> {}

/// SAFETY: `<T as Borrow<T>>::borrow` is the identity function
unsafe impl<T: ?Sized> MaybeBorrowed<T> for T {}
/// SAFETY: `<&T as Borrow<T>>::borrow` just derefs the pointer
unsafe impl<T: ?Sized> MaybeBorrowed<T> for &T {}

/// The values stored in the buffers, returned by [`Writer::split`]
#[non_exhaustive]
pub struct Split<'a, T, Extras: ?Sized> {
    pub read: &'a T,
    pub write: &'a T,
    pub extras: &'a Extras,
}

/// The values stored in the buffers, returned by [`Writer::split_mut`]
#[non_exhaustive]
pub struct SplitMut<'a, T, Extras: ?Sized> {
    pub read: &'a T,
    pub write: &'a mut T,
    pub extras: &'a Extras,
}
