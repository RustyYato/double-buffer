//! This is a thin wrapper around another strategy which puts the `WriterId` into the same allocation as the strategy
//! so that `raw::Writer` is smaller (it doesn't need to carry around the `WriterId`).

use core::cell::UnsafeCell;

use crate::interface::Strategy;

pub struct OutlineWriterStrategy<S: Strategy> {
    writer_id: UnsafeCell<S::WriterId>,
    strategy: S,
}

// SAFETY: From a shared reference to `OutlineWriterStrategy<S>` you can get
// * a shared reference to S
// * a shared reference to S::WriterId
// * an exclusive reference to S::WriterId
unsafe impl<S: Strategy + Sync> Sync for OutlineWriterStrategy<S> where S::WriterId: Send + Sync {}

#[non_exhaustive]
pub struct OutlineWriterId;

impl<S: Strategy> OutlineWriterStrategy<S> {
    pub fn new(mut strategy: S) -> Self {
        Self {
            // SAFETY: Struct drop order ensures that the writer id is dropped before the strategy
            writer_id: UnsafeCell::new(unsafe { strategy.create_writer_id() }),
            strategy,
        }
    }

    fn writer_id<'a>(&'a self, _writer: &'a OutlineWriterId) -> &'a S::WriterId {
        // SAFETY: all users of writer_id (in this module) are safe
        // because they all require a valid `&'a OutlineWriterId`, which was
        // created by this `OutlineWriterStrategy`. This ensures that we are allowed
        // to access this writer_id.
        unsafe { &*self.writer_id.get() }
    }

    fn writer_id_mut<'a>(&'a self, _writer: &'a mut OutlineWriterId) -> &'a mut S::WriterId {
        // SAFETY: all users of writer_id (in this module) are safe
        // because they all require a valid `&'a mut OutlineWriterId`, which was
        // created by this `OutlineWriterStrategy`. This ensures that we are allowed
        // to access this writer_id.
        unsafe { &mut *self.writer_id.get() }
    }
}

/// SAFETY: defer to the safety of S, since all methods defer to `S`
unsafe impl<S: Strategy> Strategy for OutlineWriterStrategy<S> {
    type WriterId = OutlineWriterId;
    type ReaderId = S::ReaderId;
    type Swap = S::Swap;
    type SwapError = S::SwapError;
    type ReadGuard = S::ReadGuard;

    unsafe fn create_writer_id(&mut self) -> Self::WriterId {
        OutlineWriterId
    }

    unsafe fn create_reader_id_from_writer(&self, writer: &Self::WriterId) -> Self::ReaderId {
        // SAFETY: defer to S::create_reader_id_from_writer
        unsafe {
            self.strategy
                .create_reader_id_from_writer(self.writer_id(writer))
        }
    }

    unsafe fn create_reader_id_from_reader(&self, reader: &Self::ReaderId) -> Self::ReaderId {
        // SAFETY: defer to S::create_reader_id_from_reader
        unsafe { self.strategy.create_reader_id_from_reader(reader) }
    }

    fn create_invalid_reader_id() -> Self::ReaderId {
        S::create_invalid_reader_id()
    }

    unsafe fn is_swapped_writer(&self, writer: &Self::WriterId) -> bool {
        // SAFETY: defer to S::is_swapped_writer
        unsafe { self.strategy.is_swapped_writer(self.writer_id(writer)) }
    }

    unsafe fn is_swapped(&self, reader: &mut Self::ReaderId, guard: &Self::ReadGuard) -> bool {
        // SAFETY: defer to S::is_swapped
        unsafe { self.strategy.is_swapped(reader, guard) }
    }

    unsafe fn try_start_swap(
        &self,
        writer: &mut Self::WriterId,
    ) -> Result<Self::Swap, Self::SwapError> {
        // SAFETY: defer to S::try_start_swap
        unsafe { self.strategy.try_start_swap(self.writer_id_mut(writer)) }
    }

    unsafe fn is_swap_finished(&self, writer: &mut Self::WriterId, swap: &mut Self::Swap) -> bool {
        // SAFETY: defer to S::is_swap_finished
        unsafe {
            self.strategy
                .is_swap_finished(self.writer_id_mut(writer), swap)
        }
    }

    unsafe fn acquire_read_guard(&self, reader: &mut Self::ReaderId) -> Self::ReadGuard {
        // SAFETY: defer to S::acquire_read_guard
        unsafe { self.strategy.acquire_read_guard(reader) }
    }

    unsafe fn release_read_guard(&self, reader: &mut Self::ReaderId, guard: Self::ReadGuard) {
        // SAFETY: defer to S::release_read_guard
        unsafe { self.strategy.release_read_guard(reader, guard) }
    }
}
