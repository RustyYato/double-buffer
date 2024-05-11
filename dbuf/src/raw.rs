use core::{cell::UnsafeCell, ops};

mod reader;
mod writer;

pub use reader::Reader;
pub use writer::Writer;

pub struct DoubleBufferData<T, S, Extras: ?Sized = ()> {
    buffers: DoubleBufferCell<T>,
    pub strategy: S,
    pub extras: Extras,
}

#[repr(transparent)]
struct DoubleBufferCell<T> {
    parts: [UnsafeCell<T>; 2],
}

impl<T> DoubleBufferCell<T> {
    const fn get(&self, swapped: bool) -> (*const T, *mut T) {
        (
            self.parts[(!swapped) as usize].get(),
            self.parts[(swapped) as usize].get(),
        )
    }
}

impl<T, S> DoubleBufferData<T, S> {
    #[inline]
    pub const fn new(back: T, front: T, strategy: S) -> Self {
        Self::with_extras(back, front, strategy, ())
    }
}

impl<T, S, Extras> DoubleBufferData<T, S, Extras> {
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

/// A copy of [`alloc::borrow::Cow`] but specialized for just `Clone` types
///
/// It doesn't have the same extensive api, and is only used to avoid
/// an atomic increment when reading from the buffer
pub enum Cow<'a, T> {
    Borrowed(&'a T),
    Owned(T),
}

impl<T> ops::Deref for Cow<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        match self {
            Cow::Borrowed(x) => x,
            Cow::Owned(x) => x,
        }
    }
}

#[non_exhaustive]
pub struct Split<'a, T, Extras: ?Sized> {
    pub read: &'a T,
    pub write: &'a T,
    pub extras: &'a Extras,
}

#[non_exhaustive]
pub struct SplitMut<'a, T, Extras: ?Sized> {
    pub read: &'a T,
    pub write: &'a mut T,
    pub extras: &'a Extras,
}
