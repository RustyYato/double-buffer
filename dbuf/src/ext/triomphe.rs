use crate::{
    interface::{
        DoubleBufferReaderPointer, DoubleBufferWriterPointer, IntoDoubleBufferWriterPointer,
        Strategy,
    },
    raw::{Cow, DoubleBufferData},
};

use triomphe::{Arc, OffsetArc, UniqueArc};

// SAFETY: UniqueArc is guaranteed to not be aliased
// and will point to the same value as the Arc created from UniqueArc::shareable
// And Arc::into_raw_offset will point to the same value as it's argument
// so by transitivity into_writer will point to the same value
unsafe impl<T, S: Strategy, Extras> IntoDoubleBufferWriterPointer
    for UniqueArc<DoubleBufferData<T, S, Extras>>
{
    type Writer = OffsetArc<DoubleBufferData<T, S, Extras>>;

    type Strategy = S;
    type Buffer = T;
    type Extras = Extras;

    fn into_writer(self) -> Self::Writer {
        Arc::into_raw_offset(self.shareable())
    }
}

// SAFETY: Self::deref does not change which [`DoubleBufferData`] it points to
// Self::reader -> try_reader will return self
unsafe impl<T, S: Strategy, Extras> DoubleBufferWriterPointer
    for OffsetArc<DoubleBufferData<T, S, Extras>>
{
    type Reader = Self;

    type Strategy = S;
    type Buffer = T;
    type Extras = Extras;

    #[inline]
    fn reader(&self) -> Self::Reader {
        self.clone()
    }
}

// SAFETY: as long as the only usage of this type is through try_writer;
// * multiple calls to try_writer must yield the same writer
//  Cow::Borrowed does always points to the same value
// * once try_writer returns [`Err`], it must never return [`Ok`] again
//  try_writer never returns [`Err`], so this condition isn't relevant
unsafe impl<T, S: Strategy, Extras> DoubleBufferReaderPointer
    for OffsetArc<DoubleBufferData<T, S, Extras>>
{
    type Writer = Self;

    type Strategy = S;
    type Buffer = T;
    type Extras = Extras;

    type UpgradeError = core::convert::Infallible;

    #[inline]
    fn try_writer(&self) -> Result<Cow<'_, Self::Writer>, Self::UpgradeError> {
        Ok(Cow::Borrowed(self))
    }
}
