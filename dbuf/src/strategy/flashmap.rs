//! this strategy was inspired by the flashmap crate
//!
//! see [`flashmap`](https://docs.rs/flashmap/latest/flashmap/) for more details

use core::{
    cell::Cell,
    mem::MaybeUninit,
    sync::atomic::{AtomicIsize, AtomicUsize, Ordering},
    task::Poll,
};
use std::{
    sync::{Mutex, Once, PoisonError},
    thread::Thread,
};

use crate::interface::{AsyncStrategy, BlockingStrategy, Strategy};

use alloc::vec::Vec;
use triomphe::Arc;

#[cfg(test)]
mod test;

pub struct ThreadParkToken(Cell<Option<Thread>>);
pub struct AsyncParkToken(atomic_waker::AtomicWaker);
pub struct AdaptiveStrategy {
    thread_token: ThreadParkToken,
    async_token: AsyncParkToken,
}

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
};

// SAFETY: FlashStrategy ensures that all access to the park token
// by the writer only happens when the residual is negative
// and by readers when the residual is zero (and by only one reader)
//
// These two states are mutually disjoint, so they cannot race
// All other parts of the FlashStrategy are trivially thread-safe
unsafe impl Sync for ThreadParkToken {}

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

pub struct Swap {
    residual: isize,
}

impl FlashStrategy<ThreadParkToken> {
    pub const fn new() -> Self {
        Self::with_park_token()
    }
}

impl FlashStrategy<AsyncParkToken> {
    pub const fn new_async() -> Self {
        Self::with_park_token()
    }
}

impl FlashStrategy<AdaptiveStrategy> {
    pub const fn new_adaptive() -> Self {
        Self::with_park_token()
    }
}

impl<ParkToken: self::Parker> FlashStrategy<ParkToken> {
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

mod seal {
    pub trait Seal {}
}

/// This is an internal trait, do not use it directly.
///
/// It only exists as an implementation detail to abstract over ThreadParkToken and AsyncParkToken
///
/// This trait is sealed, so you cannot implement this trait.
///
/// # Safety
///
/// ParkToken::wait must not unwind
pub unsafe trait Parker: Sized + seal::Seal {
    #[doc(hidden)]
    const NEW: Self;

    #[doc(hidden)]
    unsafe fn wake(&self);
}

impl seal::Seal for ThreadParkToken {}
// # SAFETY: thread::park doesn not unwind
unsafe impl Parker for ThreadParkToken {
    #[doc(hidden)]
    const NEW: Self = ThreadParkToken(Cell::new(None));

    #[doc(hidden)]
    unsafe fn wake(&self) {
        if let Some(thread) = self.0.take() {
            thread.unpark()
        }
    }
}

impl seal::Seal for AsyncParkToken {}
// # SAFETY: thread::park doesn not unwind
unsafe impl Parker for AsyncParkToken {
    #[doc(hidden)]
    const NEW: Self = AsyncParkToken(atomic_waker::AtomicWaker::new());

    #[doc(hidden)]
    unsafe fn wake(&self) {
        self.0.wake()
    }
}

impl seal::Seal for AdaptiveStrategy {}
// # SAFETY: thread::park doesn not unwind
unsafe impl Parker for AdaptiveStrategy {
    #[doc(hidden)]
    const NEW: Self = AdaptiveStrategy {
        thread_token: ThreadParkToken::NEW,
        async_token: AsyncParkToken::NEW,
    };

    #[doc(hidden)]
    unsafe fn wake(&self) {
        // SAFETY: ensured by caller
        unsafe {
            self.thread_token.wake();
            self.async_token.wake();
        }
    }
}

impl Swap {
    // This negation cannot overflow because swap.residual is always positive
    // and -isize::MAX does not overflow
    #[inline]
    #[allow(clippy::arithmetic_side_effects)]
    const fn expected_residual(&self) -> isize {
        -self.residual
    }
}

// SAFETY: FlashStrategy when used as a strategy for a double buffer is thread safe
// because finish_swap doesn't return while there are any readers in the
// buffer that the writer (even if the readers are on other threads). see the module
// docs for more information on the particular algorithm.
unsafe impl<ParkToken: self::Parker> Strategy for FlashStrategy<ParkToken> {
    type WriterId = WriterId;
    type ReaderId = ReaderId;

    type Swap = Swap;
    type SwapError = core::convert::Infallible;

    type ReadGuard = ReadGuard;

    #[inline]
    fn create_writer_id(&mut self) -> Self::WriterId {
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
        static mut INVALID: MaybeUninit<Arc<AtomicUsize>> = MaybeUninit::uninit();
        static ONCE: Once = Once::new();

        // SAFETY: ONCE ensures that there are no races on the write to INVALID
        // and that INVALID is initialized before ONCE.call_once_force completes
        ONCE.call_once_force(|_| unsafe {
            INVALID = MaybeUninit::new(Arc::new(AtomicUsize::new(0)));
        });

        // SAFETY: INVALID was initialized just above in the ONCE
        let arc = unsafe { &*INVALID.as_ptr() };

        ReaderId { id: arc.clone() }
    }

    unsafe fn is_swapped_writer(&self, _writer: &Self::WriterId) -> bool {
        // SAFETY: The only write to self.swap_state happens in try_start_swap
        // which needs a &mut Self::WriterId, but we current hold a &Self::WriterId.
        //
        // There are at most 1 Self::WriterId's associated with a given strategy at a time.
        //
        // So there must be some syncronization between this and `try_start_swap`.
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
            if Arc::is_unique(reader) {
                return false;
            }

            let reader_swap_state = reader.load(Ordering::Acquire);

            // This increment is bounded by the number of readers there are
            // which can never exceed isize::MAX (because of the max allocation
            // size of readers) so this increment can never overflow
            #[allow(clippy::arithmetic_side_effects)]
            if reader_swap_state == residual_swap_state {
                residual += 1;
            }

            true
        });

        Ok(Swap { residual })
    }

    unsafe fn is_swap_finished(&self, _writer: &mut Self::WriterId, swap: &mut Self::Swap) -> bool {
        self.residual.load(Ordering::Acquire) == swap.expected_residual()
    }

    unsafe fn acquire_read_guard(&self, reader: &mut Self::ReaderId) -> Self::ReadGuard {
        let swap_state = self.swap_state.load(Ordering::Acquire);
        let reader_id = &reader.id;
        assert_eq!(
            reader_id.load(Ordering::Relaxed) & READER_ACTIVE,
            0,
            "Detected a leaked read guard"
        );
        reader_id.store(swap_state | READER_ACTIVE, Ordering::Release);
        ReadGuard { swap_state }
    }

    unsafe fn release_read_guard(&self, reader: &mut Self::ReaderId, _guard: Self::ReadGuard) {
        let reader_swap_state =
            reader.id.fetch_xor(READER_ACTIVE, Ordering::Release) ^ READER_ACTIVE;
        let swap_state = self.swap_state.load(Ordering::Acquire);

        // if there wasn't any intervening swap then just return
        if swap_state == reader_swap_state {
            return;
        }

        // if was an intervening swap, then this is a residual reader
        // from the last swap. So we should register it as such

        let residual = self.residual.fetch_sub(1, Ordering::AcqRel);

        // if there are more resiudal readers, then someone else will wake up the writer
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
        swap: &mut Self::Swap,
        ctx: &mut core::task::Context<'_>,
    ) -> Poll<()> {
        self.poll(swap, |should_set| {
            if should_set {
                self.parker.0.register(ctx.waker())
            } else {
                self.parker.0.take();
            }
        })
    }
}

// SAFETY: we check if is_swap_finished would return true before returning
unsafe impl BlockingStrategy for FlashStrategy<ThreadParkToken> {
    unsafe fn finish_swap(&self, _writer: &mut Self::WriterId, mut swap: Self::Swap) {
        if self
            .poll(&mut swap, |should_set| {
                if should_set {
                    self.parker.0.set(Some(std::thread::current()))
                } else {
                    self.parker.0.set(None);
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

// SAFETY: we check if is_swap_finished would return true before returning Poll::Ready
unsafe impl AsyncStrategy for FlashStrategy<AdaptiveStrategy> {
    unsafe fn register_context(
        &self,
        _writer: &mut Self::WriterId,
        swap: &mut Self::Swap,
        ctx: &mut core::task::Context<'_>,
    ) -> Poll<()> {
        self.poll(swap, |should_set| {
            if should_set {
                self.parker.async_token.0.register(ctx.waker())
            } else {
                self.parker.async_token.0.take();
            }
        })
    }
}

// SAFETY: we check if is_swap_finished would return true before returning
unsafe impl BlockingStrategy for FlashStrategy<AdaptiveStrategy> {
    unsafe fn finish_swap(&self, _writer: &mut Self::WriterId, mut swap: Self::Swap) {
        if self
            .poll(&mut swap, |should_set| {
                if should_set {
                    self.parker.thread_token.0.set(Some(std::thread::current()))
                } else {
                    self.parker.thread_token.0.set(None);
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
    fn poll(&self, swap: &mut Swap, mut setup: impl FnMut(bool)) -> Poll<()> {
        if self.residual.load(Ordering::Acquire) == swap.expected_residual() {
            if swap.residual != 0 {
                self.residual.fetch_add(swap.residual, Ordering::Release);
            }
            return Poll::Ready(());
        }

        let expected_residual = swap.expected_residual();
        let residual = core::mem::take(&mut swap.residual);
        setup(true);
        let residual = self.residual.fetch_add(residual, Ordering::Release);
        // if all residual readers finished already
        if residual == expected_residual {
            setup(false);
            return Poll::Ready(());
        }

        Poll::Pending
    }
}
