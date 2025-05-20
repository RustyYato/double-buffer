use crate::interface::{
    self as iface, AsyncStrategy, BlockingStrategy, DoubleBufferWriterPointer,
    IntoDoubleBufferWriterPointer, Strategy, WriterId,
};

use super::{reader::Reader, Split, SplitMut};

/// A writer to a double buffer
///
/// see [`raw`](super) module level docs for details on usage
pub struct Writer<
    P: DoubleBufferWriterPointer,
    // use this "useless" pointer to regain covariance in the strategy and extras
    S: Strategy = <P as DoubleBufferWriterPointer>::Strategy,
> {
    id: WriterId<S>,
    ptr: P,
}

/// Create a new [`Writer`]
pub fn new_writer<T: IntoDoubleBufferWriterPointer>(mut ptr: T) -> Writer<T::Writer> {
    // SAFETY: The writer id is dropped before the pointer, and the pointer keeps the strategy
    // alive
    let id = unsafe { ptr.strategy.create_writer_id() };
    let ptr = ptr.into_writer();

    Writer { id, ptr }
}

impl<P: DoubleBufferWriterPointer> Writer<P> {
    /// Create a new writer using the given unique buffer pointer
    pub fn new<T: IntoDoubleBufferWriterPointer<Writer = P>>(ptr: T) -> Self {
        new_writer(ptr)
    }

    /// Create a new reader that points to the same buffers as this writer
    pub fn reader(&self) -> Reader<P::Reader> {
        // SAFETY: the writer id is valid
        let id = unsafe { self.ptr.strategy.create_reader_id_from_writer(&self.id) };
        // SAFETY: the reader id was just created, so it's valid
        unsafe { Reader::from_raw_parts(id, self.ptr.reader()) }
    }

    /// Get a shared reference to the writer half of the double buffer
    #[inline]
    pub fn get(&self) -> &P::Buffer {
        self.split().write
    }

    /// Get an exclusive reference to the writer half of the double buffer
    #[inline]
    pub fn get_mut(&mut self) -> &mut P::Buffer {
        self.split_mut().write
    }

    /// Get an extra data stored along-side the buffers
    #[inline]
    pub fn extras(&self) -> &P::Extras {
        &self.ptr.extras
    }

    /// Get shared references to both buffers
    #[inline]
    pub fn split(&self) -> Split<P::Buffer, P::Extras> {
        let dbuf = &*self.ptr;

        // SAFETY: self.id is valid (invariant of Self)
        let swapped = unsafe { dbuf.strategy.is_swapped_writer(&self.id) };

        let (read, write) = dbuf.buffers.get(swapped);

        // SAFETY: read and write are both valid for reads, and a shared reference can't race with
        // readers
        unsafe {
            Split {
                read: &*read,
                write: &*write,
                extras: &dbuf.extras,
            }
        }
    }

    /// Get a shared reference to the reader-half and an exclusive reference to the writer half of
    /// the buffers
    #[inline]
    pub fn split_mut(&mut self) -> SplitMut<P::Buffer, P::Extras> {
        let dbuf = &*self.ptr;

        // SAFETY: self.id is valid (invariant of Self)
        let swapped = unsafe { dbuf.strategy.is_swapped_writer(&self.id) };

        let (read, write) = dbuf.buffers.get(swapped);

        // SAFETY: read and write are both valid for reads, and a shared reference can't race with
        // readers
        // The readers can't access the write buffer, and we have an exclusive reference to self
        // so no one else can access the write buffer
        unsafe {
            SplitMut {
                read: &*read,
                write: &mut *write,
                extras: &dbuf.extras,
            }
        }
    }

    /// Try to swap the buffers, if the swap fails returns an error
    ///
    /// See the underlying strategy for details on when this may fail
    pub fn try_swap(&mut self) -> Result<(), iface::SwapError<P::Strategy>>
    where
        P::Strategy: BlockingStrategy,
    {
        // SAFETY: there are no calls to split_mut or get_mut in this function
        // and we immediately call finish_swap, which cannot unwind, so there are no
        // code paths, including panic code paths which can lead to a call to split_mut
        // or get_mut without finish_swap completing
        let swap = unsafe { self.try_start_swap()? };
        // SAFETY: the swap is the latest swap
        unsafe { self.finish_swap(swap) }
        Ok(())
    }

    /// Try to swap the buffers
    ///
    /// # Panics
    ///
    /// If the buffer swap fails for some reason, then this function will panic
    ///
    /// See the underlying strategy for details on when this may fail
    pub fn swap(&mut self)
    where
        P::Strategy: BlockingStrategy,
        iface::SwapError<P::Strategy>: core::fmt::Debug,
    {
        fn swap_failed<E: core::fmt::Debug>(err: E) -> ! {
            panic!("swap failed: {err:?}")
        }

        if let Err(err) = self.try_swap() {
            swap_failed(err)
        }
    }

    /// Try to start a buffer swap, returns an error if it's not possible
    ///
    /// See the underlying strategy for details on when this may fail
    ///
    /// Note: you should not call `try_start_swap` twice, while it is not UB,
    /// it may lead to unpredictiable behaviors, such as panics, dead-locks, and more.
    ///
    /// If you find yourself reaching for `try_start_swap`, instead try using `DelayWriter`,
    /// which provides a safe interface for `try_start_swap`
    ///
    /// # Safety
    ///
    /// there should be no calls to [`Self::split_mut`], [`Self::get_mut`], or [`Self::try_start_swap`] until
    /// [`Self::is_swap_finished`] returns true, [`Self::finish_swap`] is called
    /// or [`Self::afinish_swap`] is driven to completion
    pub unsafe fn try_start_swap(
        &mut self,
    ) -> Result<iface::Swap<P::Strategy>, iface::SwapError<P::Strategy>> {
        // SAFETY: the writer id is valid (invariant of Self)
        unsafe { self.ptr.strategy.try_start_swap(&mut self.id) }
    }

    /// Check if the given swap is completed
    ///
    /// # Safety
    ///
    /// this swap should be the latest one created from [`Self::try_start_swap`]
    pub unsafe fn is_swap_finished(&mut self, swap: &mut iface::Swap<P::Strategy>) -> bool {
        // SAFETY: guaranteed by caller
        unsafe { self.ptr.strategy.is_swap_finished(&mut self.id, swap) }
    }

    /// Finish an ongoing swap
    ///
    /// If you are using an async strategy, use [`Self::afinish_swap`]
    ///
    /// NOTE: This future must be driven to completion before you can call
    /// [`Self::split_mut`] or [`Self::get_mut`]
    ///
    /// # Safety
    ///
    /// this swap should be the latest one created from [`Self::try_start_swap`]
    pub unsafe fn finish_swap(&mut self, swap: iface::Swap<P::Strategy>)
    where
        P::Strategy: BlockingStrategy,
    {
        let no_unwind = NoUnwind;

        // SAFETY: guaranteed by caller
        // NoUnwind guarantees that all panics are converted to aborts
        unsafe { self.ptr.strategy.finish_swap(&mut self.id, swap) }

        core::mem::forget(no_unwind);
    }

    /// Try to finish a swap
    ///
    /// If you are using a blocking strategy, use [`Self::finish_swap`]
    ///
    /// # Safety
    ///
    /// this swap should be the latest one created from try_start_swap
    pub unsafe fn afinish_swap<'a, 's>(
        &'a mut self,
        swap: &'s mut iface::Swap<P::Strategy>,
    ) -> WaitForSwap<'a, 's, P::Strategy>
    where
        P::Strategy: AsyncStrategy,
    {
        WaitForSwap {
            strategy: &self.ptr.strategy,
            swap,
            id: &mut self.id,
        }
    }
}

struct NoUnwind;

impl Drop for NoUnwind {
    fn drop(&mut self) {
        panic!("detected unwind while finishing a swap, this is a critical bug which cannot be recovered from")
    }
}

/// A future which can be awaited to ensure that the swap is completed
pub struct WaitForSwap<'a, 's, S: AsyncStrategy> {
    strategy: &'a S,
    swap: &'s mut S::Swap,
    id: &'a mut S::WriterId,
}

impl<S: AsyncStrategy> core::future::Future for WaitForSwap<'_, '_, S> {
    type Output = ();

    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Self::Output> {
        let no_unwind = NoUnwind;

        // SAFETY: a pin on Self does not pin any of it's fields
        let this = core::pin::Pin::into_inner(self);
        // SAFETY: the id can from a Writer and the swap is the latest swap
        // and while this future is alive, no one else can create a new swap
        // because we have exclusive access to the writer
        // If this future is dropped before completion, that's OK
        // the strategy should be able to handle multiple calls to
        // try_start_swap before any call to finish_swap
        let out = unsafe {
            if this.strategy.is_swap_finished(this.id, this.swap) {
                core::task::Poll::Ready(())
            } else {
                this.strategy.register_context(this.id, this.swap, cx)
            }
        };

        core::mem::forget(no_unwind);

        out
    }
}
