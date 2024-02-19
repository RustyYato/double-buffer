use core::cell::UnsafeCell;

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
