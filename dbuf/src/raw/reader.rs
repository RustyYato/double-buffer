use core::{marker::PhantomData, mem::ManuallyDrop, ops, ptr::NonNull};

use super::Cow;

use crate::interface::{
    self as iface, create_invalid_reader_id, DoubleBufferReaderPointer, DoubleBufferWriterPointer,
    ReaderId, Strategy,
};

pub struct Reader<P, S: Strategy = <P as DoubleBufferReaderPointer>::Strategy> {
    id: ReaderId<S>,
    ptr: P,
}

pub struct ReaderGuard<'a, T: ?Sized, P: DoubleBufferWriterPointer> {
    ptr: NonNull<T>,
    raw: RawReaderGuard<'a, P>,
    lt: PhantomData<&'a T>,
}

struct RawReaderGuard<'a, P: DoubleBufferWriterPointer> {
    guard: ManuallyDrop<iface::ReaderGuard<P::Strategy>>,
    reader_id: &'a mut ReaderId<P::Strategy>,
    writer: Cow<'a, P>,
}

impl<P: DoubleBufferWriterPointer> Drop for RawReaderGuard<'_, P> {
    fn drop(&mut self) {
        let guard = unsafe { ManuallyDrop::take(&mut self.guard) };
        unsafe {
            self.writer
                .strategy
                .release_read_guard(self.reader_id, guard)
        }
    }
}

impl<P: DoubleBufferReaderPointer> Reader<P> {
    #[inline]
    pub(crate) unsafe fn from_raw_parts(id: ReaderId<P::Strategy>, ptr: P) -> Self {
        Self { id, ptr }
    }

    pub fn try_read(&mut self) -> Result<ReaderGuard<'_, P::Buffer, P::Writer>, P::UpgradeError> {
        let ptr = self.ptr.try_writer()?;
        let guard = unsafe { ptr.strategy.acquire_read_guard(&mut self.id) };
        let swapped = unsafe { ptr.strategy.is_swapped(&guard) };

        let (reader, _) = ptr.buffers.get(swapped);

        Ok(ReaderGuard {
            ptr: unsafe { NonNull::new_unchecked(reader.cast_mut()) },
            raw: RawReaderGuard {
                guard: ManuallyDrop::new(guard),
                reader_id: &mut self.id,
                writer: ptr,
            },
            lt: PhantomData,
        })
    }

    pub fn read(&mut self) -> ReaderGuard<'_, P::Buffer, P::Writer>
    where
        P::UpgradeError: core::fmt::Debug,
    {
        fn read_failed<T: core::fmt::Debug>(err: &T) -> ! {
            panic!("Cannot access a dropped double buffer: {err:?}")
        }

        match self.try_read() {
            Ok(guard) => guard,
            Err(err) => read_failed(&err),
        }
    }
}

impl<P: DoubleBufferReaderPointer> Clone for Reader<P> {
    #[inline]
    fn clone(&self) -> Self {
        let id = match self.ptr.try_writer() {
            Ok(ptr) => unsafe { ptr.strategy.create_reader_id_from_reader(&self.id) },
            Err(_) => create_invalid_reader_id::<P::Strategy>(),
        };

        unsafe { Self::from_raw_parts(id, self.ptr.clone()) }
    }
}

impl<T: ?Sized, P: DoubleBufferWriterPointer> ops::Deref for ReaderGuard<'_, T, P> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        unsafe { self.ptr.as_ref() }
    }
}

impl<'a, T: ?Sized, P: DoubleBufferWriterPointer> ReaderGuard<'a, T, P> {
    pub fn try_map<U: ?Sized, E>(
        self,
        f: impl FnOnce(&T) -> Result<&U, E>,
    ) -> Result<ReaderGuard<'a, U, P>, (Self, E)> {
        match f(&self) {
            Ok(ptr) => Ok(ReaderGuard {
                ptr: NonNull::from(ptr),
                raw: self.raw,
                lt: PhantomData,
            }),
            Err(err) => Err((self, err)),
        }
    }

    pub fn map<U: ?Sized>(self, f: impl FnOnce(&T) -> &U) -> ReaderGuard<'a, U, P> {
        match self.try_map::<_, core::convert::Infallible>(move |t| Ok(f(t))) {
            Ok(guard) => guard,
            Err((_, err)) => match err {},
        }
    }
}
