use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::interface::{AsyncStrategy, BlockingStrategy, Strategy};

const MAX_READER_COUNT: u64 = u64::MAX / 2;

#[cfg(test)]
mod tests;

pub struct AtomicStrategy {
    num_readers: [AtomicU64; 2],
    which: AtomicBool,
}

impl AtomicStrategy {
    #[inline]
    pub const fn new() -> Self {
        Self {
            num_readers: [AtomicU64::new(0), AtomicU64::new(0)],
            which: AtomicBool::new(false),
        }
    }
}

impl Default for AtomicStrategy {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY:
//
// If there are no readers currently reading from the buffer
// then we can swap to that buffer. If there are any readers reading
// from the buffer an error is returned, and no swap happens
unsafe impl Strategy for AtomicStrategy {
    type WriterId = ();
    type ReaderId = ();

    type Swap = ();
    type SwapError = ();

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
        // SAFETY: The caller ensures that the writer id is valid,
        // and since the only write to `self.which` is in `try_start_swap`
        // there is no race with reading the value here
        unsafe { core::ptr::read(&self.which) }.into_inner()
    }

    #[inline]
    unsafe fn is_swapped(&self, _reader: &mut Self::ReaderId, _guard: &Self::ReadGuard) -> bool {
        self.which.load(Ordering::Acquire)
    }

    #[inline]
    unsafe fn try_start_swap(
        &self,
        writer: &mut Self::WriterId,
    ) -> Result<Self::Swap, Self::SwapError> {
        // SAFETY: The caller ensures that the writer id is valid
        let next_swap = unsafe { !self.is_swapped_writer(writer) };

        if self.num_readers[next_swap as usize].load(Ordering::Acquire) != 0 {
            Err(())
        } else {
            self.which.store(next_swap, Ordering::Release);
            Ok(())
        }
    }

    #[inline]
    unsafe fn is_swap_finished(
        &self,
        _writer: &mut Self::WriterId,
        _swap: &mut Self::Swap,
    ) -> bool {
        true
    }

    #[inline]
    unsafe fn acquire_read_guard(&self, _reader: &mut Self::ReaderId) -> Self::ReadGuard {
        let swapped = self.which.load(Ordering::Acquire);
        let num_readers = &self.num_readers[swapped as usize];
        let readers = num_readers.fetch_add(1, Ordering::Acquire);

        if readers > MAX_READER_COUNT {
            num_readers.fetch_sub(1, Ordering::Release);

            fn tried_to_read_too_many_times() -> ! {
                panic!("Tried to read too many times")
            }

            tried_to_read_too_many_times()
        }

        swapped
    }

    #[inline]
    unsafe fn release_read_guard(&self, _reader: &mut Self::ReaderId, guard: Self::ReadGuard) {
        let swapped = guard;
        let num_readers = &self.num_readers[swapped as usize];
        num_readers.fetch_sub(1, Ordering::Release);
    }
}

// SAFETY: is_swap_finished always returns true
unsafe impl AsyncStrategy for AtomicStrategy {
    #[inline]
    unsafe fn register_context(
        &self,
        _writer: &mut Self::WriterId,
        _swap: &mut Self::Swap,
        _ctx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<()> {
        core::task::Poll::Ready(())
    }
}

// SAFETY: is_swap_finished always returns true
unsafe impl BlockingStrategy for AtomicStrategy {
    #[inline]
    unsafe fn finish_swap(&self, _writer: &mut Self::WriterId, _swap: Self::Swap) {}
}
