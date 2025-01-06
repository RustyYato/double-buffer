//! this strategy was inspired by the flashmap crate
//!
//! see [`flashmap`](https://docs.rs/flashmap/latest/flashmap/) for more details

use core::{
    sync::atomic::{AtomicIsize, AtomicUsize, Ordering},
    task::Poll,
};
use std::sync::{Mutex, OnceLock, PoisonError};

use crate::interface::{AsyncStrategy, BlockingStrategy, Strategy};

use alloc::vec::Vec;
use triomphe::Arc;

use super::flash_park_token::{AdaptiveParkToken, AsyncParkToken, Parker, ThreadParkToken};

#[cfg(test)]
mod test;

pub struct FlashStrategy<ParkToken> {
    swap_state: AtomicUsize,
    readers: Mutex<Vec<Arc<AtomicUsize>>>,
    residual: AtomicIsize,
    parker: ParkToken,
}

const _: () = {
    const fn send_sync<T: Send + Sync>() {}

    let _ = send_sync::<FlashStrategy<ThreadParkToken>>;
    let _ = send_sync::<FlashStrategy<AsyncParkToken>>;
    let _ = send_sync::<FlashStrategy<AdaptiveParkToken>>;
};

const NOT_SWAPPED: usize = 0;
const SWAPPED: usize = 1;
const READER_ACTIVE: usize = 2;

pub struct WriterId(());
pub struct ReaderId {
    id: Arc<AtomicUsize>,
}

pub struct ReadGuard {
    swap_state: usize,
}

impl FlashStrategy<ThreadParkToken> {
    pub const fn new_blocking() -> Self {
        Self::with_park_token()
    }
}

impl FlashStrategy<AsyncParkToken> {
    pub const fn new_async() -> Self {
        Self::with_park_token()
    }
}

impl FlashStrategy<AdaptiveParkToken> {
    pub const fn new() -> Self {
        Self::with_park_token()
    }
}

impl Default for FlashStrategy<ThreadParkToken> {
    #[inline]
    fn default() -> Self {
        Self::new_blocking()
    }
}

impl Default for FlashStrategy<AsyncParkToken> {
    #[inline]
    fn default() -> Self {
        Self::new_async()
    }
}

impl Default for FlashStrategy<AdaptiveParkToken> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<ParkToken: Parker> FlashStrategy<ParkToken> {
    const fn with_park_token() -> Self {
        Self {
            swap_state: AtomicUsize::new(NOT_SWAPPED),
            readers: Mutex::new(Vec::new()),
            residual: AtomicIsize::new(0),
            parker: ParkToken::NEW,
        }
    }
}

impl<ParkToken> FlashStrategy<ParkToken> {
    fn create_reader_id(&self) -> ReaderId {
        let mut readers = self.readers.lock().unwrap_or_else(PoisonError::into_inner);
        let reader = Arc::new(AtomicUsize::new(0));
        readers.push(reader.clone());
        ReaderId { id: reader }
    }
}

#[non_exhaustive]
pub struct Swap;

// SAFETY: FlashStrategy when used as a strategy for a double buffer is thread safe
// because finish_swap doesn't return while there are any readers in the
// buffer that the writer (even if the readers are on other threads). see the module
// docs for more information on the particular algorithm.
unsafe impl<ParkToken: Parker> Strategy for FlashStrategy<ParkToken> {
    type WriterId = WriterId;
    type ReaderId = ReaderId;

    type Swap = Swap;
    type SwapError = core::convert::Infallible;

    type ReadGuard = ReadGuard;

    #[inline]
    unsafe fn create_writer_id(&mut self) -> Self::WriterId {
        WriterId(())
    }

    #[inline]
    unsafe fn create_reader_id_from_writer(&self, _writer: &Self::WriterId) -> Self::ReaderId {
        self.create_reader_id()
    }

    #[inline]
    unsafe fn create_reader_id_from_reader(&self, _reader: &Self::ReaderId) -> Self::ReaderId {
        self.create_reader_id()
    }

    #[cold]
    #[inline(never)]
    fn create_invalid_reader_id() -> Self::ReaderId {
        static INVALID: OnceLock<Arc<AtomicUsize>> = OnceLock::new();

        let invalid = INVALID.get_or_init(|| Arc::new(AtomicUsize::new(0)));

        ReaderId {
            id: invalid.clone(),
        }
    }

    unsafe fn is_swapped_writer(&self, _writer: &Self::WriterId) -> bool {
        // SAFETY: The only write to self.swap_state happens in try_start_swap
        // which needs a &mut Self::WriterId, but we current hold a &Self::WriterId.
        //
        // There are at most 1 Self::WriterId's associated with a given strategy at a time.
        //
        // So there must be some synchronization between this and `try_start_swap`.
        // So there can be no race between that write and this read.
        //
        // And it is fine to race two (non-atomic) reads
        let swap_state = unsafe { core::ptr::read(&self.swap_state).into_inner() };
        swap_state != NOT_SWAPPED
    }

    unsafe fn is_swapped(&self, _reader: &mut Self::ReaderId, guard: &Self::ReadGuard) -> bool {
        guard.swap_state != NOT_SWAPPED
    }

    unsafe fn try_start_swap(
        &self,
        _writer: &mut Self::WriterId,
    ) -> Result<Self::Swap, Self::SwapError> {
        let old_swap_state = self.swap_state.fetch_xor(SWAPPED, Ordering::Release);

        let mut readers = self.readers.lock().unwrap_or_else(PoisonError::into_inner);

        let residual_swap_state = old_swap_state | READER_ACTIVE;
        let mut residual = 0;

        readers.retain(|reader| {
            // if the reader was dropped, then remove it from the list
            if Arc::is_unique(reader) {
                return false;
            }

            // swap the buffers in each reader
            let reader_swap_state = reader.fetch_xor(1, Ordering::AcqRel);

            // This increment is bounded by the number of readers there are
            // which can never exceed isize::MAX (because of the max allocation
            // size of readers) so this increment can never overflow
            #[allow(clippy::arithmetic_side_effects)]
            if reader_swap_state == residual_swap_state {
                residual += 1;
            }

            true
        });

        self.residual.fetch_add(residual, Ordering::Release);

        Ok(Swap)
    }

    unsafe fn is_swap_finished(&self, _writer: &mut Self::WriterId, Swap: &mut Self::Swap) -> bool {
        self.residual.load(Ordering::Acquire) == 0
    }

    unsafe fn acquire_read_guard(&self, reader: &mut Self::ReaderId) -> Self::ReadGuard {
        let reader_id = &*reader.id;

        assert_eq!(
            reader_id.load(Ordering::Relaxed) & READER_ACTIVE,
            0,
            "Detected a leaked read guard"
        );

        let id = reader_id.fetch_or(READER_ACTIVE, Ordering::Release);
        ReadGuard { swap_state: id }
    }

    unsafe fn release_read_guard(&self, reader: &mut Self::ReaderId, guard: Self::ReadGuard) {
        let reader_swap_state = reader.id.fetch_and(!READER_ACTIVE, Ordering::Release);

        // if there wasn't any intervening swap then just return
        if guard.swap_state & 1 == reader_swap_state & 1 {
            return;
        }

        // if was an intervening swap, then this is a residual reader
        // from the last swap. So we should register it as such

        let residual = self.residual.fetch_sub(1, Ordering::AcqRel);

        // if there are more residual readers, then someone else will wake up the writer
        if residual != 1 {
            return;
        }

        // if this is the last residual reader, then wake up the writer

        // SAFETY: residual is non-zero
        unsafe { self.parker.wake() }
    }
}

// SAFETY: we check if is_swap_finished would return true before returning Poll::Ready
unsafe impl AsyncStrategy for FlashStrategy<AsyncParkToken> {
    unsafe fn register_context(
        &self,
        _writer: &mut Self::WriterId,
        Swap: &mut Self::Swap,
        ctx: &mut core::task::Context<'_>,
    ) -> Poll<()> {
        self.poll(|should_set| {
            if should_set {
                self.parker.set(ctx)
            } else {
                self.parker.clear();
            }
        })
    }
}

#[cfg(feature = "std")]
// SAFETY: we check if is_swap_finished would return true before returning
unsafe impl BlockingStrategy for FlashStrategy<ThreadParkToken> {
    unsafe fn finish_swap(&self, _writer: &mut Self::WriterId, Swap: Self::Swap) {
        if self
            .poll(|should_set| {
                if should_set {
                    self.parker.set()
                } else {
                    self.parker.clear();
                }
            })
            .is_pending()
        {
            while self.residual.load(Ordering::Relaxed) != 0 {
                std::thread::park();
            }
        }
    }
}

#[cfg(feature = "std")]
// SAFETY: we check if is_swap_finished would return true before returning Poll::Ready
unsafe impl AsyncStrategy for FlashStrategy<AdaptiveParkToken> {
    unsafe fn register_context(
        &self,
        _writer: &mut Self::WriterId,
        Swap: &mut Self::Swap,
        ctx: &mut core::task::Context<'_>,
    ) -> Poll<()> {
        self.poll(|should_set| {
            if should_set {
                self.parker.async_token.set(ctx)
            } else {
                self.parker.async_token.clear();
            }
        })
    }
}

// SAFETY: we check if is_swap_finished would return true before returning
unsafe impl BlockingStrategy for FlashStrategy<AdaptiveParkToken> {
    unsafe fn finish_swap(&self, _writer: &mut Self::WriterId, Swap: Self::Swap) {
        if self
            .poll(|should_set| {
                if should_set {
                    self.parker.thread_token.set()
                } else {
                    self.parker.thread_token.clear();
                }
            })
            .is_pending()
        {
            while self.residual.load(Ordering::Relaxed) != 0 {
                std::thread::park();
            }
        }
    }
}

impl<T> FlashStrategy<T> {
    fn poll(&self, mut setup: impl FnMut(bool)) -> Poll<()> {
        if self.residual.load(Ordering::Acquire) == 0 {
            return Poll::Ready(());
        }

        setup(true);
        let residual = self.residual.load(Ordering::Acquire);
        // if all residual readers finished already
        if residual == 0 {
            setup(false);
            return Poll::Ready(());
        }

        Poll::Pending
    }
}
