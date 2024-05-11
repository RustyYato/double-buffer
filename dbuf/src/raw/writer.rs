use crate::interface::{
    self as iface, AsyncStrategy, BlockingStrategy, DoubleBufferWriterPointer,
    IntoDoubleBufferWriterPointer, Strategy, WriterId,
};

use super::{reader::Reader, Split, SplitMut};

pub struct Writer<
    P: DoubleBufferWriterPointer,
    // use this "useless" pointer to regain covariance in the strategy and extras
    S: Strategy = <P as DoubleBufferWriterPointer>::Strategy,
> {
    id: WriterId<S>,
    ptr: P,
}

pub fn new_writer<T: IntoDoubleBufferWriterPointer>(mut ptr: T) -> Writer<T::Writer> {
    let id = ptr.strategy.create_writer_id();
    let ptr = ptr.into_writer();

    Writer { id, ptr }
}

impl<P: DoubleBufferWriterPointer> Writer<P> {
    pub fn new<T: IntoDoubleBufferWriterPointer<Writer = P>>(ptr: T) -> Self {
        new_writer(ptr)
    }

    pub fn reader(&self) -> Reader<P::Reader> {
        // SAFETY: the writer id is valid
        let id = unsafe { self.ptr.strategy.create_reader_id_from_writer(&self.id) };
        // SAFETY: the reader id was just created, so it's valid
        unsafe { Reader::from_raw_parts(id, self.ptr.reader()) }
    }

    #[inline]
    pub fn get(&self) -> &P::Buffer {
        self.split().write
    }

    #[inline]
    pub fn get_mut(&mut self) -> &mut P::Buffer {
        self.split_mut().write
    }

    #[inline]
    pub fn extras(&self) -> &P::Extras {
        &self.ptr.extras
    }

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

    pub fn try_swap(&mut self) -> Result<(), iface::SwapError<P::Strategy>>
    where
        P::Strategy: BlockingStrategy,
    {
        // SAFETY: there are no calls to split_mut or get_mut in this function
        // and we immediately call finish_swap, which cannot unwind, so there are no
        // code paths, inclduing panic code paths which can lead to a call to split_mut
        // or get_mut without finish_swap completing
        let swap = unsafe { self.try_start_swap()? };
        // SAFETY: the swap is the latest swap
        unsafe { self.finish_swap(swap) }
        Ok(())
    }

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

    /// # Safety
    ///
    /// [`Self::finish_swap`] must be called or [`Self::afinish_swap`] must be polled to completion
    /// before you can call [`Self::split_mut`] or [`Self::get_mut`]
    ///
    /// # Safety
    ///
    /// there should be no calls to [`Self::split_mut`] or [`Self::get_mut`] until
    /// [`Self::is_swap_finished`] returns true, [`Self::finish_swap`] is called
    /// or [`Self::afinish_swap`] is driven to completion
    pub unsafe fn try_start_swap(
        &mut self,
    ) -> Result<iface::Swap<P::Strategy>, iface::SwapError<P::Strategy>> {
        // SAFETY: the writer id is valid (invariant of Self)
        unsafe { self.ptr.strategy.try_start_swap(&mut self.id) }
    }

    /// # Safety
    ///
    /// this swap should be the latest one created from [`Self::try_start_swap`]
    pub unsafe fn is_swap_finished(&mut self, swap: &mut iface::Swap<P::Strategy>) -> bool {
        // SAFETY: guaranteed by caller
        unsafe { self.ptr.strategy.is_swap_finished(&mut self.id, swap) }
    }

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

    /// # Safety
    ///
    /// This swap should be the latest one created from [`Self::try_start_swap`]
    ///
    /// This future should be driven to completion before calling any mutable methods on self
    pub async unsafe fn afinish_swap(&mut self, mut swap: iface::Swap<P::Strategy>)
    where
        P::Strategy: AsyncStrategy,
    {
        // SAFETY: the caller ensures that this is the latests swap
        unsafe { self.try_afinish_swap(&mut swap).await };
    }

    /// # Safety
    ///
    /// this swap should be the latest one created from try_start_swap
    ///
    /// This future should be driven to completion before calling any mutable methods on self
    /// or this the swap should be completed via one of the other methods
    /// ([`Self::afinish_swap`], [`Self::finish_swap`])
    pub async unsafe fn try_afinish_swap(&mut self, swap: &mut iface::Swap<P::Strategy>)
    where
        P::Strategy: AsyncStrategy,
    {
        struct WaitForSwap<'a, S: AsyncStrategy> {
            strategy: &'a S,
            swap: &'a mut S::Swap,
            id: &'a mut S::WriterId,
        }

        impl<S: AsyncStrategy> core::future::Future for WaitForSwap<'_, S> {
            type Output = ();

            fn poll(
                self: core::pin::Pin<&mut Self>,
                cx: &mut core::task::Context<'_>,
            ) -> core::task::Poll<Self::Output> {
                // SAFETY: a pin on Self does not pin any of it's fields
                let this = core::pin::Pin::into_inner(self);
                // SAFETY: the id can from a Writer and the swap is the latest swap
                // and while this future is alive, no one else can create a new swap
                // because we have exclusive access to the writer
                // If this future is dropped before completion, that's OK
                // the strategy should be able to handle multiple calls to
                // try_start_swap before any call to finish_swap
                unsafe {
                    if this.strategy.is_swap_finished(this.id, this.swap) {
                        core::task::Poll::Ready(())
                    } else {
                        this.strategy.register_context(this.id, this.swap, cx)
                    }
                }
            }
        }

        impl<S: AsyncStrategy> Drop for WaitForSwap<'_, S> {
            fn drop(&mut self) {
                // SAFETY: this self.id is valid and swap was created from this id
                unsafe {
                    while !self.strategy.is_swap_finished(self.id, self.swap) {
                        core::hint::spin_loop()
                    }
                }
            }
        }

        let no_unwind = NoUnwind;

        WaitForSwap {
            strategy: &self.ptr.strategy,
            swap,
            id: &mut self.id,
        }
        .await;

        core::mem::forget(no_unwind);
    }
}

struct NoUnwind;

impl Drop for NoUnwind {
    fn drop(&mut self) {
        panic!("detected unwind while finishing a swap, this is a critical bug which cannot be recovered from")
    }
}
