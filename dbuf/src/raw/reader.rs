use core::{borrow::Borrow, mem::ManuallyDrop, ops};

use crate::interface::{
    self as iface, create_invalid_reader_id, DoubleBufferReaderPointer, DoubleBufferWriterPointer,
    ReaderId, Strategy,
};

use super::reference::RawReference;

/// A reader into a double buffer
///
/// This is initially created from [`Writer::reader`](crate::raw::Writer::reader), but
/// can then be cloned as much as you need.
pub struct Reader<P, S: Strategy = <P as DoubleBufferReaderPointer>::Strategy> {
    id: ReaderId<S>,
    ptr: P,
}

/// A guard into the double buffer. As long as this guard is alive, the writer
/// cannot write to the corresponding buffer.
pub struct ReaderGuard<'a, T: ?Sized, P: DoubleBufferWriterPointer> {
    ptr: RawReference<'a, T>,
    extras: RawReference<'a, P::Extras>,
    raw: RawReaderGuard<'a, P>,
}

struct RawReaderGuard<'a, P: 'a + DoubleBufferWriterPointer> {
    guard: ManuallyDrop<iface::ReaderGuard<P::Strategy>>,
    reader_id: &'a mut ReaderId<P::Strategy>,
    writer: <P::Reader as DoubleBufferReaderPointer>::MaybeBorrowed<'a>,
}

impl<P: Copy + DoubleBufferReaderPointer> Copy for Reader<P> where ReaderId<P::Strategy>: Copy {}

impl<P: DoubleBufferWriterPointer> core::panic::UnwindSafe for RawReaderGuard<'_, P> {}
impl<P: DoubleBufferWriterPointer> core::panic::RefUnwindSafe for RawReaderGuard<'_, P> {}
impl<P: DoubleBufferWriterPointer> core::marker::Unpin for RawReaderGuard<'_, P> {}

impl<P: DoubleBufferWriterPointer> Drop for RawReaderGuard<'_, P> {
    fn drop(&mut self) {
        // SAFETY: self.guard isn't dropped before this (in fact, it's not even access between
        // construction and here)
        let guard = unsafe { ManuallyDrop::take(&mut self.guard) };
        // SAFETY: the reader id was set by a valid reader and self.writer
        // ensures that the strategy wasn't dropped or granted exclusive access elsewhere
        // so the reader id must still be value (no one else is allowed to call
        // Strategy::create_writer_id)
        unsafe {
            self.writer
                .borrow()
                .strategy
                .release_read_guard(self.reader_id, guard)
        }
    }
}

impl<P: DoubleBufferReaderPointer> Reader<P> {
    /// Create a new reader from an id and pointer
    #[inline]
    pub(crate) const unsafe fn from_raw_parts(id: ReaderId<P::Strategy>, ptr: P) -> Self {
        Self { id, ptr }
    }

    /// Try to access the read buffer, if it fails then returns an error
    ///
    /// see the pointer's docs for when upgrading the pointer can fail
    pub fn try_read(&mut self) -> Result<ReaderGuard<'_, P::Buffer, P::Writer>, P::UpgradeError> {
        let ptr = self.ptr.try_writer()?;
        let data = ptr.borrow();
        // SAFETY: the reader id is valid (this is an invariant of Self)
        let guard = unsafe { data.strategy.acquire_read_guard(&mut self.id) };
        // SAFETY: the guard was created from the given reader id, and is the latest guard
        let swapped = unsafe { data.strategy.is_swapped(&mut self.id, &guard) };

        let (reader, _) = data.buffers.get(swapped);

        let extras = core::ptr::addr_of!(data.extras);

        Ok(ReaderGuard {
            // SAFETY: the pointer from ptr.buffers.get are always non-null
            ptr: unsafe { RawReference::new(reader.cast_mut()) },
            // SAFETY: references are always non-null, and extras is derived from a reference
            extras: unsafe { RawReference::new(extras.cast_mut()) },
            raw: RawReaderGuard {
                guard: ManuallyDrop::new(guard),
                reader_id: &mut self.id,
                writer: ptr,
            },
        })
    }

    /// Try to access the read buffer
    ///
    /// # Panic
    ///
    /// If upgrading the pointer fails, this will panic
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
            // SAFETY: the reader id is valid (this is an invariant of Self)
            Ok(ptr) => unsafe { ptr.borrow().strategy.create_reader_id_from_reader(&self.id) },
            Err(_) => create_invalid_reader_id::<P::Strategy>(),
        };

        // SAFETY: id is valid for the strategy inside ptr
        // or the ptr is dead and the reader id is invalid
        unsafe { Self::from_raw_parts(id, self.ptr.clone()) }
    }
}

impl<T: ?Sized, P: DoubleBufferWriterPointer> ops::Deref for ReaderGuard<'_, T, P> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: self.raw ensures that the writer doesn't have access to self.ptr
        // so there is no race with the writer, and readers cannot race with each other
        // self.ptr is non-null, well aligned, allocated and valid for reads
        &self.ptr
    }
}

impl<'a, T: ?Sized, P: DoubleBufferWriterPointer> ReaderGuard<'a, T, P> {
    pub const fn extras(&self) -> &P::Extras {
        // SAFETY: extras is derived from a reference, which is bound to the lifetime
        // 'a, so it is still valid.
        self.extras.as_ref()
    }

    /// Try to map the [`ReaderGuard`] to another value
    pub fn try_map<U: ?Sized, E>(
        self,
        f: impl FnOnce(&T) -> Result<&U, E>,
    ) -> Result<ReaderGuard<'a, U, P>, (Self, E)> {
        self.try_map_with_extras(|value, _| f(value))
    }

    /// Map the [`ReaderGuard`] to another value
    pub fn map<U: ?Sized>(self, f: impl FnOnce(&T) -> &U) -> ReaderGuard<'a, U, P> {
        match self.try_map::<_, core::convert::Infallible>(move |t| Ok(f(t))) {
            Ok(guard) => guard,
            Err((_, err)) => match err {},
        }
    }

    /// Try to map the [`ReaderGuard`] to another value
    pub fn try_map_with_extras<U: ?Sized, E>(
        self,
        f: impl for<'t> FnOnce(&'t T, &'t P::Extras) -> Result<&'t U, E>,
    ) -> Result<ReaderGuard<'a, U, P>, (Self, E)> {
        match f(&self, self.extras()) {
            Ok(ptr) => Ok(ReaderGuard {
                // SAFETY: this pointer is derived from self, which is guaranteed to be live for `'a`
                ptr: unsafe { RawReference::new(core::ptr::from_ref(ptr).cast_mut()) },
                extras: self.extras,
                raw: self.raw,
            }),
            Err(err) => Err((self, err)),
        }
    }

    /// Map the [`ReaderGuard`] to another value
    pub fn map_with_extras<U: ?Sized>(
        self,
        f: impl for<'t> FnOnce(&'t T, &'t P::Extras) -> &'t U,
    ) -> ReaderGuard<'a, U, P> {
        match self.try_map_with_extras::<_, core::convert::Infallible>(move |t, e| Ok(f(t, e))) {
            Ok(guard) => guard,
            Err((_, err)) => match err {},
        }
    }
}
