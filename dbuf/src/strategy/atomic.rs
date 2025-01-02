use const_fn::const_fn;
#[cfg(not(loom))]
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(loom)]
use loom::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::interface::{AsyncStrategy, BlockingStrategy, Strategy};

#[cfg(test)]
mod tests;

pub struct AtomicStrategy {
    num_readers: [AtomicU64; 2],
    which: AtomicBool,
}

impl AtomicStrategy {
    #[inline]
    #[const_fn(cfg(not(loom)))]
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

    type Swap = bool;
    type SwapError = ();

    type ReadGuard = bool;

    #[inline]
    unsafe fn create_writer_id(&mut self) -> Self::WriterId {}

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
        #[cfg(not(loom))]
        unsafe {
            core::ptr::read(&self.which).into_inner()
        }
        // SAFETY: The caller ensures that the writer id is valid,
        // and since the only write to `self.which` is in `try_start_swap`
        // there is no race with reading the value here
        #[cfg(loom)]
        unsafe {
            self.which.unsync_load()
        }
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

        Ok(next_swap)
    }

    #[inline]
    unsafe fn is_swap_finished(
        &self,
        _writer: &mut Self::WriterId,
        &mut next_swap: &mut Self::Swap,
    ) -> bool {
        let num_readers = &self.num_readers[next_swap as usize];

        // lock the number of readers
        if num_readers
            .compare_exchange(0, u64::MAX, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            self.which.store(next_swap, Ordering::Release);
            num_readers.store(0, Ordering::Release);
            true
        } else {
            false
        }
    }

    #[inline]
    unsafe fn acquire_read_guard(&self, _reader: &mut Self::ReaderId) -> Self::ReadGuard {
        let mut swapped = !self.which.load(Ordering::Acquire);
        let mut reader_count = &self.num_readers[swapped as usize];

        let mut num_readers = reader_count.load(Ordering::Acquire);

        loop {
            #[cfg(loom)]
            loom::thread::yield_now();

            let Some(next_num_readers) = num_readers.checked_add(1) else {
                // the writer locked the readers and swapped the buffers
                // so refresh everything

                swapped = !self.which.load(Ordering::Acquire);
                reader_count = &self.num_readers[swapped as usize];
                num_readers = reader_count.load(Ordering::Acquire);

                core::hint::spin_loop();
                continue;
            };

            match reader_count.compare_exchange_weak(
                num_readers,
                next_num_readers,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return swapped,
                Err(current) => num_readers = current,
            }

            core::hint::spin_loop();
        }
    }

    #[inline]
    unsafe fn release_read_guard(&self, _reader: &mut Self::ReaderId, guard: Self::ReadGuard) {
        let swapped = guard;
        let num_readers = &self.num_readers[swapped as usize];
        num_readers.fetch_sub(1, Ordering::Release);
    }
}

// // SAFETY: is_swap_finished always returns true
// unsafe impl AsyncStrategy for AtomicStrategy {
//     #[inline]
//     unsafe fn register_context(
//         &self,
//         _writer: &mut Self::WriterId,
//         _swap: &mut Self::Swap,
//         _ctx: &mut core::task::Context<'_>,
//     ) -> core::task::Poll<()> {
//         core::task::Poll::Ready(())
//     }
// }
