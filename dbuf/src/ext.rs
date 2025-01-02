use crate::{
    interface::{
        DoubleBufferReaderPointer, DoubleBufferWriterPointer, IntoDoubleBufferWriterPointer,
        Strategy,
    },
    raw::DoubleBufferData,
};

#[cfg(feature = "triomphe")]
mod triomphe;

#[cfg(feature = "alloc")]
mod std_arc;

#[cfg(feature = "alloc")]
mod std_rc;

// SAFETY: &mut is exclusive, and it is undefined behavior for it to alias with any other pointer
// &mut _ -> &_ cast preserves which value it points to
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

// SAFETY: Self::deref does not change which [`DoubleBufferData`] it points to
// Self::reader -> try_reader will return self
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

// SAFETY: as long as the only usage of this type is through try_writer;
// * multiple calls to try_writer must yield the same writer
//   try_writer always returns self
// * once try_writer returns [`Err`], it must never return [`Ok`] again
//   try_writer never returns [`Err`]
unsafe impl<T, S: Strategy, Extras: ?Sized> DoubleBufferReaderPointer
    for &DoubleBufferData<T, S, Extras>
{
    type Writer = Self;

    type Strategy = S;
    type Buffer = T;
    type Extras = Extras;
    type UpgradeError = core::convert::Infallible;
    type MaybeBorrowed<'a>
        = Self
    where
        Self: 'a;

    #[inline]
    fn try_writer(&self) -> Result<Self::MaybeBorrowed<'_>, Self::UpgradeError> {
        Ok(self)
    }
}
