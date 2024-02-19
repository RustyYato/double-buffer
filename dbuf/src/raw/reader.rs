use crate::interface::{
    create_invalid_reader_id, DoubleBufferReaderPointer, ReaderId, Strategy, WriterId,
};

pub struct Reader<P, S: Strategy = <P as DoubleBufferReaderPointer>::Strategy> {
    id: ReaderId<S>,
    ptr: P,
}

impl<P: DoubleBufferReaderPointer> Reader<P> {
    #[inline]
    pub(crate) unsafe fn from_raw_parts(id: ReaderId<P::Strategy>, ptr: P) -> Self {
        Self { id, ptr }
    }
}

impl<P: DoubleBufferReaderPointer> Clone for Reader<P> {
    #[inline]
    fn clone(&self) -> Self {
        let id = match self.ptr.try_writer() {
            Ok(ptr) => unsafe { ptr.strategy.create_reader_id_from_reader(&self.id) },
            Err(_) => unsafe { create_invalid_reader_id::<P::Strategy>() },
        };

        unsafe { Self::from_raw_parts(id, self.ptr.clone()) }
    }
}
