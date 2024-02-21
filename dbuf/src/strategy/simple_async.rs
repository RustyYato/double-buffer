use core::{cell::Cell, task::Waker};

use crate::interface::{AsyncStrategy, Strategy};

#[cfg(test)]
mod test;

pub struct SimpleAsyncStrategy {
    // how many readers in each buffer
    num_readers: [Cell<u32>; 2],
    swapped: Cell<bool>,
    waker: Cell<Option<Waker>>,
}

impl SimpleAsyncStrategy {
    #[inline]
    pub const fn new() -> Self {
        Self {
            num_readers: [Cell::new(0), Cell::new(0)],
            swapped: Cell::new(false),
            waker: Cell::new(None),
        }
    }
}

// SAFETY:
//
// If there are no readers currently reading from the buffer
// then we can swap to that buffer. If there are any readers reading
// from the buffer an error is returned, and no swap happens
unsafe impl Strategy for SimpleAsyncStrategy {
    type WriterId = ();
    type ReaderId = ();

    type Swap = ();
    type SwapError = core::convert::Infallible;

    type ReadGuard = bool;

    #[inline]
    fn create_writer_id(&mut self) -> Self::WriterId {}

    #[inline]
    unsafe fn create_reader_id_from_writer(&self, _writer: &Self::WriterId) -> Self::ReaderId {}

    #[inline]
    unsafe fn create_reader_id_from_reader(&self, _reader: &Self::ReaderId) -> Self::ReaderId {}

    #[inline]
    fn create_invalid_reader_id() -> Self::ReaderId {}

    #[inline]
    unsafe fn is_swapped_writer(&self, _writer: &Self::WriterId) -> bool {
        self.swapped.get()
    }

    #[inline]
    unsafe fn is_swapped(&self, _reader: &mut Self::ReaderId, _guard: &Self::ReadGuard) -> bool {
        self.swapped.get()
    }

    #[inline]
    unsafe fn try_start_swap(
        &self,
        _writer: &mut Self::WriterId,
    ) -> Result<Self::Swap, Self::SwapError> {
        let next_swap = !self.swapped.get();
        self.swapped.set(next_swap);
        Ok(())
    }

    #[inline]
    unsafe fn is_swap_finished(
        &self,
        _writer: &mut Self::WriterId,
        _swap: &mut Self::Swap,
    ) -> bool {
        self.num_readers[self.swapped.get() as usize].get() == 0
    }

    #[inline]
    unsafe fn acquire_read_guard(&self, _reader: &mut Self::ReaderId) -> Self::ReadGuard {
        let swapped = !self.swapped.get();
        let num_readers = &self.num_readers[swapped as usize];
        num_readers.set(
            num_readers
                .get()
                .checked_add(1)
                .expect("too many readers reading at once"),
        );
        swapped
    }

    #[inline]
    unsafe fn release_read_guard(&self, _reader: &mut Self::ReaderId, guard: Self::ReadGuard) {
        let swapped = guard;
        let num_readers = &self.num_readers[swapped as usize];
        num_readers.set(num_readers.get().wrapping_sub(1));
        if num_readers.get() == 0 {
            if let Some(waker) = self.waker.take() {
                waker.wake()
            }
        }
    }
}

impl AsyncStrategy for SimpleAsyncStrategy {
    #[inline]
    unsafe fn register_context(
        &self,
        _writer: &mut Self::WriterId,
        _swap: &mut Self::Swap,
        ctx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<()> {
        self.waker.set(Some(ctx.waker().clone()));
        core::task::Poll::Pending
    }
}
