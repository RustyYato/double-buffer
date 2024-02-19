//! this strategy was inspired by the flashmap crate
//!
//! see [`flashmap`](https://docs.rs/flashmap/latest/flashmap/) for more details

use core::{
    cell::Cell,
    mem::MaybeUninit,
    sync::atomic::{AtomicIsize, AtomicUsize, Ordering},
};
use std::{
    sync::{Mutex, Once, PoisonError},
    thread::Thread,
};

use crate::interface::Strategy;

use slab::Slab;
use triomphe::Arc;

mod test;

pub struct FlashStrategy {
    swap_state: AtomicUsize,
    readers: Mutex<Slab<Arc<AtomicUsize>>>,
    residual: AtomicIsize,
    park_token: Cell<Option<Thread>>,
}

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

impl FlashStrategy {
    pub const fn new() -> Self {
        Self {
            swap_state: AtomicUsize::new(NOT_SWAPPED),
            readers: Mutex::new(Slab::new()),
            residual: AtomicIsize::new(0),
            park_token: Cell::new(None),
        }
    }

    fn create_reader_id(&self) -> ReaderId {
        let mut readers = self.readers.lock().unwrap_or_else(PoisonError::into_inner);
        let reader = Arc::new(AtomicUsize::new(0));
        readers.insert(reader.clone());
        ReaderId { id: reader }
    }
}

unsafe impl Strategy for FlashStrategy {
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
    unsafe fn create_invalid_reader_id() -> Self::ReaderId {
        static mut INVALID: MaybeUninit<Arc<AtomicUsize>> = MaybeUninit::uninit();
        static ONCE: Once = Once::new();

        ONCE.call_once_force(|_| {
            INVALID = MaybeUninit::new(Arc::new(AtomicUsize::new(0)));
        });

        let arc = &*INVALID.as_ptr();

        ReaderId { id: arc.clone() }
    }

    unsafe fn is_swapped_exclusive(&self, writer: &mut Self::WriterId) -> bool {
        self.is_swapped_shared(writer)
    }

    unsafe fn is_swapped_shared(&self, _writer: &Self::WriterId) -> bool {
        core::ptr::read(&self.swap_state).into_inner() != NOT_SWAPPED
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

        readers.retain(|_, reader| {
            if Arc::is_unique(reader) {
                return false;
            }

            let reader_swap_state = reader.load(Ordering::Acquire);

            if reader_swap_state == residual_swap_state {
                residual += 1;
            }

            true
        });

        Ok(Swap { residual })
    }

    unsafe fn is_swap_finished(&self, _writer: &mut Self::WriterId, swap: &mut Self::Swap) -> bool {
        self.residual.load(Ordering::Acquire) == -swap.residual
    }

    unsafe fn finish_swap(&self, _writer: &mut Self::WriterId, swap: Self::Swap) {
        if self.residual.load(Ordering::Acquire) == -swap.residual {
            self.residual.fetch_add(swap.residual, Ordering::Release);
            return;
        }

        let current = std::thread::current();
        self.park_token.set(Some(current));
        let residual = self.residual.fetch_add(swap.residual, Ordering::Release);

        // if all residual readers finished already
        if residual == -swap.residual {
            self.park_token.set(None);
            return;
        }

        std::thread::park();
    }

    unsafe fn acquire_read_guard(&self, reader: &mut Self::ReaderId) -> Self::ReadGuard {
        let swap_state = self.swap_state.load(Ordering::Acquire);
        assert_eq!(
            reader.id.load(Ordering::Relaxed) & READER_ACTIVE,
            0,
            "Detected a leaked read guard"
        );
        let reader_id = &reader.id;
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

        let token = self.park_token.take().unwrap();

        token.unpark();
    }
}
