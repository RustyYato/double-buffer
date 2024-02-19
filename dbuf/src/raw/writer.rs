use crate::interface::{
    self as iface, AsyncStrategy, DoubleBufferWriterPointer, IntoDoubleBufferWriterPointer,
    Strategy, WriterId,
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
    let id = unsafe { ptr.strategy.create_writer_id() };
    let ptr = ptr.into_writer();

    Writer { id, ptr }
}

impl<P: DoubleBufferWriterPointer> Writer<P> {
    pub fn new<T: IntoDoubleBufferWriterPointer<Writer = P>>(ptr: T) -> Self {
        new_writer(ptr)
    }

    pub fn reader(&self) -> Reader<P::Reader> {
        unsafe {
            let id = self.ptr.strategy.create_reader_id_from_writer(&self.id);
            Reader::from_raw_parts(id, self.ptr.reader())
        }
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

        let swapped = unsafe { dbuf.strategy.is_swapped_shared(&self.id) };

        let (read, write) = dbuf.buffers.get(swapped);

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

        let swapped = unsafe { dbuf.strategy.is_swapped_exclusive(&mut self.id) };

        let (read, write) = dbuf.buffers.get(swapped);

        unsafe {
            SplitMut {
                read: &*read,
                write: &mut *write,
                extras: &dbuf.extras,
            }
        }
    }

    pub fn try_swap(&mut self) -> Result<(), iface::SwapError<P::Strategy>> {
        let swap = self.try_start_swap()?;
        unsafe { self.finish_swap(swap) }
        Ok(())
    }

    pub fn swap(&mut self)
    where
        iface::SwapError<P::Strategy>: core::fmt::Debug,
    {
        fn swap_failed<E: core::fmt::Debug>(err: E) -> ! {
            panic!("swap failed: {err:?}")
        }

        if let Err(err) = self.try_swap() {
            swap_failed(err)
        }
    }

    pub async fn try_aswap(&mut self) -> Result<(), iface::SwapError<P::Strategy>>
    where
        P::Strategy: AsyncStrategy,
    {
        struct WaitForSwap<'a, S: Strategy> {
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
                let this = core::pin::Pin::into_inner(self);
                unsafe {
                    if this.strategy.is_swap_finished(this.id, this.swap) {
                        core::task::Poll::Ready(())
                    } else {
                        this.strategy.register_context(this.id, this.swap, cx);
                        core::task::Poll::Pending
                    }
                }
            }
        }

        let mut swap = self.try_start_swap()?;

        WaitForSwap {
            strategy: &self.ptr.strategy,
            swap: &mut swap,
            id: &mut self.id,
        }
        .await;

        unsafe { self.finish_swap(swap) }

        Ok(())
    }

    pub async fn aswap(&mut self)
    where
        P::Strategy: AsyncStrategy,
        iface::SwapError<P::Strategy>: core::fmt::Debug,
    {
        fn swap_failed<E: core::fmt::Debug>(err: E) -> ! {
            panic!("swap failed: {err:?}")
        }

        if let Err(err) = self.try_aswap().await {
            swap_failed(err)
        }
    }

    pub fn try_start_swap(
        &mut self,
    ) -> Result<iface::Swap<P::Strategy>, iface::SwapError<P::Strategy>> {
        unsafe { self.ptr.strategy.try_start_swap(&mut self.id) }
    }

    pub unsafe fn is_swap_finished(&mut self, swap: &mut iface::Swap<P::Strategy>) -> bool {
        unsafe { self.ptr.strategy.is_swap_finished(&mut self.id, swap) }
    }

    pub unsafe fn finish_swap(&mut self, swap: iface::Swap<P::Strategy>) {
        struct NoUnwind;

        impl Drop for NoUnwind {
            fn drop(&mut self) {
                panic!("detected unwind while finishing a swap, this is a critical bug which cannot be recovered from")
            }
        }

        let no_unwind = NoUnwind;

        unsafe { self.ptr.strategy.finish_swap(&mut self.id, swap) }

        core::mem::forget(no_unwind);
    }
}
