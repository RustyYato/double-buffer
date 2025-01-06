//! this strategy was inspired by the evmap crate
//!
//! see [`evmap`](https://docs.rs/evmap/latest/evmap/) for more details

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, OnceLock, PoisonError};

use crate::interface::{BlockingStrategy, Strategy};

use alloc::vec::Vec;
use triomphe::Arc;

pub struct EvMapStrategy {
    is_swapped: AtomicBool,
    epochs: Mutex<Vec<Arc<AtomicUsize>>>,
    condvar: Condvar,
}

const _: () = {
    const fn send_sync<T: Send + Sync>() {}

    let _ = send_sync::<EvMapStrategy>;
};

pub struct WriterId {
    last_epochs: Vec<usize>,
}
pub struct ReaderId {
    id: Arc<AtomicUsize>,
}

impl EvMapStrategy {
    pub const fn new() -> Self {
        Self {
            is_swapped: AtomicBool::new(false),
            epochs: Mutex::new(Vec::new()),
            condvar: Condvar::new(),
        }
    }
}

impl Default for EvMapStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl EvMapStrategy {
    fn create_reader_id(&self) -> ReaderId {
        let mut readers = self.epochs.lock().unwrap_or_else(PoisonError::into_inner);
        let reader = Arc::new(AtomicUsize::new(0));
        readers.push(reader.clone());
        ReaderId { id: reader }
    }
}

pub struct Swap {
    range: core::ops::Range<usize>,
}

// SAFETY: FlashStrategy when used as a strategy for a double buffer is thread safe
// because finish_swap doesn't return while there are any readers in the
// buffer that the writer (even if the readers are on other threads). see the module
// docs for more information on the particular algorithm.
unsafe impl Strategy for EvMapStrategy {
    type WriterId = WriterId;
    type ReaderId = ReaderId;

    type Swap = Swap;
    type SwapError = core::convert::Infallible;

    type ReadGuard = bool;

    #[inline]
    unsafe fn create_writer_id(&mut self) -> Self::WriterId {
        WriterId {
            last_epochs: Vec::new(),
        }
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
        unsafe { core::ptr::read(&self.is_swapped).into_inner() }
    }

    unsafe fn is_swapped(&self, _reader: &mut Self::ReaderId, guard: &Self::ReadGuard) -> bool {
        *guard
    }

    unsafe fn try_start_swap(
        &self,
        writer: &mut Self::WriterId,
    ) -> Result<Self::Swap, Self::SwapError> {
        self.is_swapped.fetch_xor(true, Ordering::AcqRel);

        let mut epochs = self.epochs.lock().unwrap_or_else(PoisonError::into_inner);

        // retain all non-unique readers
        epochs.retain(|epoch| !Arc::is_unique(epoch));
        writer.last_epochs.resize(epochs.len(), 0);

        for (epoch, last_epoch) in epochs.iter().zip(&mut writer.last_epochs) {
            // This needs to syncronize with [acquire|release]_read_guard (so needs `Acquire`)
            *last_epoch = epoch.load(Ordering::Acquire);
        }

        Ok(Swap {
            range: 0..epochs.len(),
        })
    }

    unsafe fn is_swap_finished(&self, writer: &mut Self::WriterId, swap: &mut Self::Swap) -> bool {
        let epochs = self.epochs.lock().unwrap_or_else(PoisonError::into_inner);
        is_swap_finished(&epochs, writer, swap)
    }

    unsafe fn acquire_read_guard(&self, reader: &mut Self::ReaderId) -> Self::ReadGuard {
        // this needs to syncronize with `try_start_swap`/`is_swap_finished` (so needs at least `Release`) and
        // it needs to prevent reads from the `raw::ReaderGuard` from being reordered before this (so needs at least `Acquire`)
        // the cheapest ordering which satisfies this is `AcqRel`
        reader.id.fetch_add(1, Ordering::AcqRel);
        self.is_swapped.load(Ordering::Acquire)
    }

    unsafe fn release_read_guard(&self, reader: &mut Self::ReaderId, _guard: Self::ReadGuard) {
        // this needs to syncronize with `try_start_swap`/`is_swap_finished` (so needs at least `Release`) and
        // it needs to prevent reads from the `raw::ReaderGuard` from being reordered after this (so needs at least `Release`)
        // the cheapest ordering which satisfies this is `Release`
        reader.id.fetch_add(1, Ordering::Release);
        self.condvar.notify_one();
    }
}

// SAFETY: we check if is_swap_finished would return true before returning
unsafe impl BlockingStrategy for EvMapStrategy {
    unsafe fn finish_swap(&self, writer: &mut Self::WriterId, mut swap: Self::Swap) {
        let mut epochs = self.epochs.lock().unwrap_or_else(PoisonError::into_inner);

        while !is_swap_finished(&epochs, writer, &mut swap) {
            epochs = self
                .condvar
                .wait(epochs)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

fn is_swap_finished(epochs: &[Arc<AtomicUsize>], writer: &WriterId, swap: &mut Swap) -> bool {
    for (i, (epoch, last_epoch)) in core::iter::zip(
        &epochs[swap.range.clone()],
        &writer.last_epochs[swap.range.clone()],
    )
    .enumerate()
    {
        // if the reader wasn't reading at the start of the swap, then it cannot be in the current buffer
        if *last_epoch % 2 == 0 {
            continue;
        }

        // This needs to syncronize with [acquire|release]_read_guard (so needs `Acquire`)
        let now = epoch.load(Ordering::Acquire);

        // swap.range.start < epochs.len() - i,  so
        // swap.range.start + i < epochs.len(),  so
        // `swap.range.start + i` cannot overflow
        #[allow(clippy::arithmetic_side_effects)]
        if now == *last_epoch {
            swap.range.start += i;
            return false;
        }
    }

    true
}
