use crate::interface::{
    DoubleBufferWriterPointer, IntoDoubleBufferWriterPointer, ReaderId, Strategy, WriterId,
};

use super::reader::Reader;

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
}
