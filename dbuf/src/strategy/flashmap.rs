//! this strategy was inspired by the flashmap crate
//!
//! see [`flashmap`](https://docs.rs/flashmap/latest/flashmap/) for more details

use core::{
    cell::Cell,
    mem::MaybeUninit,
    sync::atomic::{AtomicIsize, AtomicUsize, Ordering},
    task::Waker,
};
use std::{
    sync::{Mutex, Once, PoisonError},
    thread::Thread,
};

use crate::interface::{AsyncStrategy, Strategy};

use std::vec::Vec;
use triomphe::Arc;

#[cfg(test)]
mod test;

pub struct ThreadParkToken(Thread);
pub struct AsyncParkToken(Waker);

pub struct FlashStrategy<ParkToken> {
    swap_state: AtomicUsize,
    readers: Mutex<Vec<Arc<AtomicUsize>>>,
    residual: AtomicIsize,
    park_token: Cell<Option<ParkToken>>,
}

// SAFETY: FlashStrategy doesn't use shared ownership, or thread-locals
// so it is trivially Send if the ParkToken is Send
unsafe impl<ParkToken: Send> Send for FlashStrategy<ParkToken> {}

// SAFETY: FlashStrategy ensures that all access to the park token
// by the writer only happens when the residual is negative
// and by readers when the residual is zero (and by only one reader)
//
// These two states are mutually disjoint, so they cannot race
// All other parts of the FlashStrategy are trivially thread-safe
//
unsafe impl<ParkToken: Send> Sync for FlashStrategy<ParkToken> {}

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

pub struct Swap<Store> {
    residual: isize,
    storage: Store,
}

impl FlashStrategy<ThreadParkToken> {
    pub const fn new() -> Self {
        Self::with_park_token()
    }
}

impl<ParkToken> FlashStrategy<ParkToken> {
    pub const fn with_park_token() -> Self {
        Self {
            swap_state: AtomicUsize::new(NOT_SWAPPED),
            readers: Mutex::new(Vec::new()),
            residual: AtomicIsize::new(0),
            park_token: Cell::new(None),
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

/// # Safety
///
/// ParkToken::wait must not unwind
pub unsafe trait ParkToken: Sized {
    type SwapStorage: Default;

    fn new(swap: &mut Swap<Self::SwapStorage>) -> Option<Self>;

    fn wake(self);

    fn wait();
}

// # SAFETY: thread::park doesn not unwind
unsafe impl ParkToken for ThreadParkToken {
    type SwapStorage = ();

    fn new(_writer: &mut Swap<Self::SwapStorage>) -> Option<Self> {
        Some(Self(std::thread::current()))
    }

    fn wake(self) {
        self.0.unpark()
    }

    fn wait() {
        std::thread::park()
    }
}

// # SAFETY: thread::park doesn not unwind
unsafe impl ParkToken for AsyncParkToken {
    type SwapStorage = Option<Waker>;

    fn new(swap: &mut Swap<Self::SwapStorage>) -> Option<Self> {
        swap.storage.take().map(Self)
    }

    fn wake(self) {
        self.0.wake()
    }

    fn wait() {
        loop {
            std::thread::park()
        }
    }
}

impl<T> Swap<T> {
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
unsafe impl<ParkToken: self::ParkToken> Strategy for FlashStrategy<ParkToken> {
    type WriterId = WriterId;
    type ReaderId = ReaderId;

    type Swap = Swap<ParkToken::SwapStorage>;
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

    unsafe fn is_swapped(&self, guard: &Self::ReadGuard) -> bool {
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

        Ok(Swap {
            residual,
            storage: Default::default(),
        })
    }

    unsafe fn is_swap_finished(&self, _writer: &mut Self::WriterId, swap: &mut Self::Swap) -> bool {
        self.residual.load(Ordering::Acquire) == swap.expected_residual()
    }

    unsafe fn finish_swap(&self, _writer: &mut Self::WriterId, mut swap: Self::Swap) {
        if self.residual.load(Ordering::Acquire) == swap.expected_residual() {
            self.residual.fetch_add(swap.residual, Ordering::Release);
            return;
        }

        let current = ParkToken::new(&mut swap);
        self.park_token.set(current);
        let residual = self.residual.fetch_add(swap.residual, Ordering::Release);

        // if all residual readers finished already
        if residual == swap.expected_residual() {
            self.park_token.set(None);
            return;
        }

        // FIXME: this may spuriously exit, and we should check if residual is zero before exiting
        ParkToken::wait()
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

        dbg!(swap_state, reader_swap_state);

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

        if let Some(token) = self.park_token.take() {
            token.wake();
        }
    }
}

impl AsyncStrategy for FlashStrategy<AsyncParkToken> {
    unsafe fn register_context(
        &self,
        _writer: &mut Self::WriterId,
        swap: &mut Self::Swap,
        ctx: &mut core::task::Context<'_>,
    ) {
        swap.storage = Some(ctx.waker().clone());
    }
}
