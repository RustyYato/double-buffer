use crate::{
    interface::{
        DoubleBufferReaderPointer, DoubleBufferWriterPointer, IntoDoubleBufferWriterPointer,
        Strategy,
    },
    raw::{Cow, DoubleBufferData},
};

#[cfg(feature = "triomphe")]
mod triomphe;

#[cfg(feature = "alloc")]
mod std_arc;

#[cfg(feature = "alloc")]
mod std_rc;

unsafe impl<'a, T, S: Strategy, Extras: ?Sized> IntoDoubleBufferWriterPointer
    for &'a mut DoubleBufferData<T, S, Extras>
{
    type Writer = &'a DoubleBufferData<T, S, Extras>;

    type Strategy = S;
    type Buffer = T;
    type Extras = Extras;

    #[inline]
    fn into_writer(self) -> Self::Writer {
        self
    }
}

unsafe impl<T, S: Strategy, Extras: ?Sized> DoubleBufferWriterPointer
    for &DoubleBufferData<T, S, Extras>
{
    type Reader = Self;

    type Strategy = S;
    type Buffer = T;
    type Extras = Extras;

    #[inline]
    fn reader(&self) -> Self::Reader {
        *self
    }
}

unsafe impl<T, S: Strategy, Extras: ?Sized> DoubleBufferReaderPointer
    for &DoubleBufferData<T, S, Extras>
{
    type Writer = Self;

    type Strategy = S;
    type Buffer = T;
    type Extras = Extras;
    type UpgradeError = core::convert::Infallible;

    #[inline]
    fn try_writer(&self) -> Result<Cow<'_, Self::Writer>, Self::UpgradeError> {
        Ok(Cow::Owned(self))
    }
}
