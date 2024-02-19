use crate::{
    interface::{
        DoubleBufferReaderPointer, DoubleBufferWriterPointer, IntoDoubleBufferWriterPointer,
        Strategy,
    },
    raw::{Cow, DoubleBufferData},
};

use triomphe::{Arc, OffsetArc, UniqueArc};

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
