//! this strategy was inspired by the evmap crate
//!
//! see [`evmap`](https://docs.rs/evmap/latest/evmap/) for more details

use core::{
    cell::{Cell, UnsafeCell},
    sync::atomic::{AtomicU8, AtomicUsize, Ordering},
};
use std::sync::{Mutex, OnceLock, PoisonError};

use crate::{
    interface::{AsyncStrategy, BlockingStrategy, Strategy},
    strategy::atomic::park_token::{AdaptiveParkToken, AsyncParkToken, Parker, ThreadParkToken},
};

use alloc::vec::Vec;
use triomphe::Arc;

const IS_SWAPPED: u8 = 1;
const HAS_NEW_EPOCHS: u8 = 2;

#[cfg(test)]
mod test;

struct Epoch {
    current: AtomicUsize,
    last: Cell<usize>,
}

// SAFETY: `last` is only accessed while we have a `&mut WriterId` inside `Strategy`
// which means that there can't be any races
unsafe impl Sync for Epoch {}

pub struct EvMapStrategy<P> {
    flags: AtomicU8,
    epochs: UnsafeCell<Vec<Arc<Epoch>>>,
    new_epochs_from_reader: Mutex<Vec<Arc<Epoch>>>,
    parker: P,
}

// SAFETY: we mediate access to `epochs` via `WriterId` and `Strategy`
unsafe impl<P: Sync> Sync for EvMapStrategy<P> {}

const _: () = {
    const fn _send_sync<T: Send + Sync>() {
        let _ = _send_sync::<EvMapStrategy<T>>;
    }
};

pub struct WriterId(());
pub struct ReaderId {
    epoch: Arc<Epoch>,
}

impl<P: Parker> EvMapStrategy<P> {
    pub const fn new() -> Self {
        Self {
            flags: AtomicU8::new(0),
            epochs: UnsafeCell::new(Vec::new()),
            new_epochs_from_reader: Mutex::new(Vec::new()),
            parker: P::NEW,
        }
    }

    #[allow(clippy::mut_from_ref)]
    const fn epochs(&self, _writer: &WriterId) -> &mut Vec<Arc<Epoch>> {
        // SAFETY: this function is only called inside Strategy, at which point
        // if we have a `WriterId`, then no other thread can access this code
        // and no user-defined code can run while we have access to the writer id
        unsafe { &mut *self.epochs.get() }
    }
}

impl<P: Parker> Default for EvMapStrategy<P> {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Swap {
    start: usize,
    end: usize,
}

// SAFETY: FlashStrategy when used as a strategy for a double buffer is thread safe
// because finish_swap doesn't return while there are any readers in the
// buffer that the writer (even if the readers are on other threads). see the module
// docs for more information on the particular algorithm.
unsafe impl<P: Parker> Strategy for EvMapStrategy<P> {
    type WriterId = WriterId;
    type ReaderId = ReaderId;

    type Swap = Swap;
    type SwapError = core::convert::Infallible;

    type ReadGuard = bool;

    #[inline]
    unsafe fn create_writer_id(&mut self) -> Self::WriterId {
        WriterId(())
    }

    #[inline]
    unsafe fn create_reader_id_from_writer(&self, writer: &Self::WriterId) -> Self::ReaderId {
        let readers = self.epochs(writer);
        let reader = Arc::new(Epoch {
            current: AtomicUsize::new(0),
            last: Cell::new(0),
        });
        readers.push(reader.clone());
        ReaderId { epoch: reader }
    }

    #[inline]
    unsafe fn create_reader_id_from_reader(&self, _reader: &Self::ReaderId) -> Self::ReaderId {
        let mut readers = (self.new_epochs_from_reader)
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.flags.fetch_or(HAS_NEW_EPOCHS, Ordering::Relaxed);
        let reader = Arc::new(Epoch {
            current: AtomicUsize::new(0),
            last: Cell::new(0),
        });
        readers.push(reader.clone());
        ReaderId { epoch: reader }
    }

    #[cold]
    #[inline(never)]
    fn create_invalid_reader_id() -> Self::ReaderId {
        static INVALID: OnceLock<Arc<Epoch>> = OnceLock::new();

        let invalid = INVALID.get_or_init(|| {
            Arc::new(Epoch {
                current: AtomicUsize::new(0),
                last: Cell::new(0),
            })
        });

        ReaderId {
            epoch: invalid.clone(),
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
        unsafe { core::ptr::read(&self.flags).into_inner() & IS_SWAPPED != 0 }
    }

    unsafe fn is_swapped(&self, _reader: &mut Self::ReaderId, guard: &Self::ReadGuard) -> bool {
        *guard
    }

    unsafe fn try_start_swap(
        &self,
        writer: &mut Self::WriterId,
    ) -> Result<Self::Swap, Self::SwapError> {
        let flags = self.flags.fetch_xor(IS_SWAPPED, Ordering::AcqRel);

        let epochs = self.epochs(writer);

        // if no readers cloned themselves, then we can avoid locking the
        // mutex here at minimal cost
        if flags & HAS_NEW_EPOCHS != 0 {
            let mut new_epochs = self
                .new_epochs_from_reader
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            self.flags.fetch_and(!HAS_NEW_EPOCHS, Ordering::Relaxed);

            epochs.append(&mut new_epochs);
        }

        // retain all non-unique readers
        epochs.retain(|epoch| !Arc::is_unique(epoch));

        for epoch in epochs.iter() {
            // This needs to synchronize with [acquire|release]_read_guard (so needs `Acquire`)
            epoch.last.set(epoch.current.load(Ordering::Acquire));
        }

        Ok(Swap {
            start: 0,
            end: epochs.len(),
        })
    }

    unsafe fn is_swap_finished(&self, writer: &mut Self::WriterId, swap: &mut Self::Swap) -> bool {
        is_swap_finished(self.epochs(writer), writer, swap)
    }

    unsafe fn acquire_read_guard(&self, reader: &mut Self::ReaderId) -> Self::ReadGuard {
        fn read_failed() -> ! {
            panic!("read failed: tried to read from a read handle after the read guard was leaked")
        }

        if reader.epoch.current.load(Ordering::Relaxed) % 2 != 0 {
            read_failed()
        }

        // this needs to synchronize with `try_start_swap`/`is_swap_finished` (so needs at least `Release`) and
        // it needs to prevent reads from the `raw::ReaderGuard` from being reordered before this (so needs at least `Acquire`)
        // the cheapest ordering which satisfies this is `AcqRel`
        reader.epoch.current.fetch_add(1, Ordering::AcqRel);
        self.flags.load(Ordering::Acquire) & IS_SWAPPED != 0
    }

    unsafe fn release_read_guard(&self, reader: &mut Self::ReaderId, _guard: Self::ReadGuard) {
        // this needs to synchronize with `try_start_swap`/`is_swap_finished` (so needs at least `Release`) and
        // it needs to prevent reads from the `raw::ReaderGuard` from being reordered after this (so needs at least `Release`)
        // the cheapest ordering which satisfies this is `Release`
        reader.epoch.current.fetch_add(1, Ordering::Release);
        self.parker.wake();
    }
}

// SAFETY: we check if is_swap_finished would return true before returning
unsafe impl BlockingStrategy for EvMapStrategy<ThreadParkToken> {
    unsafe fn finish_swap(&self, writer: &mut Self::WriterId, mut swap: Self::Swap) {
        let epochs = self.epochs(writer).as_slice();

        self.parker
            .park_until(|| is_swap_finished(epochs, writer, &mut swap));
    }
}

// SAFETY: we check if is_swap_finished would return true before returning
unsafe impl AsyncStrategy for EvMapStrategy<AsyncParkToken> {
    unsafe fn register_context(
        &self,
        writer: &mut Self::WriterId,
        swap: &mut Self::Swap,
        ctx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<()> {
        let epochs = self.epochs(writer);

        if is_swap_finished(epochs, writer, swap) {
            core::task::Poll::Ready(())
        } else {
            self.parker.set(ctx);
            core::task::Poll::Pending
        }
    }
}

// SAFETY: we check if is_swap_finished would return true before returning
unsafe impl BlockingStrategy for EvMapStrategy<AdaptiveParkToken> {
    unsafe fn finish_swap(&self, writer: &mut Self::WriterId, mut swap: Self::Swap) {
        let epochs = self.epochs(writer).as_slice();

        self.parker
            .thread_token
            .park_until(|| is_swap_finished(epochs, writer, &mut swap));
    }
}

// SAFETY: we check if is_swap_finished would return true before returning
unsafe impl AsyncStrategy for EvMapStrategy<AdaptiveParkToken> {
    unsafe fn register_context(
        &self,
        writer: &mut Self::WriterId,
        swap: &mut Self::Swap,
        ctx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<()> {
        let epochs = self.epochs(writer);

        if is_swap_finished(epochs, writer, swap) {
            core::task::Poll::Ready(())
        } else {
            self.parker.async_token.set(ctx);
            core::task::Poll::Pending
        }
    }
}

fn is_swap_finished(epochs: &[Arc<Epoch>], _writer: &WriterId, swap: &mut Swap) -> bool {
    for (i, epoch) in epochs[swap.start..swap.end].iter().enumerate() {
        let last_epoch = epoch.last.get();
        // if the reader wasn't reading at the start of the swap, then it cannot be in the current buffer
        if last_epoch % 2 == 0 {
            continue;
        }

        // This needs to synchronize with [acquire|release]_read_guard (so needs `Acquire`)
        let now = epoch.current.load(Ordering::Acquire);

        // swap.range.start < epochs.len() - i,  so
        // swap.range.start + i < epochs.len(),  so
        // `swap.range.start + i` cannot overflow
        #[allow(clippy::arithmetic_side_effects)]
        if now == last_epoch {
            swap.start += i;
            return false;
        }
    }

    swap.start = swap.end;

    true
}
