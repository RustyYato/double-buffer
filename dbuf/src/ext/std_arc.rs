use crate::{
    interface::{
        DoubleBufferReaderPointer, DoubleBufferWriterPointer, IntoDoubleBufferWriterPointer,
        Strategy,
    },
    raw::{Cow, DoubleBufferData},
};

use alloc::sync::{Arc, Weak};
use rc_box::ArcBox;

unsafe impl<T, S: Strategy, Extras> IntoDoubleBufferWriterPointer
    for ArcBox<DoubleBufferData<T, S, Extras>>
{
    type Writer = Arc<DoubleBufferData<T, S, Extras>>;

    type Strategy = S;
    type Buffer = T;
    type Extras = Extras;

    fn into_writer(self) -> Self::Writer {
        self.into()
    }
}

unsafe impl<T, S: Strategy, Extras: ?Sized> DoubleBufferWriterPointer
    for Arc<DoubleBufferData<T, S, Extras>>
{
    type Reader = Weak<DoubleBufferData<T, S, Extras>>;

    type Strategy = S;
    type Buffer = T;
    type Extras = Extras;

    #[inline]
    fn reader(&self) -> Self::Reader {
        Arc::downgrade(self)
    }
}

unsafe impl<T, S: Strategy, Extras: ?Sized> DoubleBufferReaderPointer
    for Weak<DoubleBufferData<T, S, Extras>>
{
    type Writer = Arc<DoubleBufferData<T, S, Extras>>;

    type Strategy = S;
    type Buffer = T;
    type Extras = Extras;

    type UpgradeError = ArcUpgradeError;

    #[inline]
    fn try_writer(&self) -> Result<Cow<'_, Self::Writer>, Self::UpgradeError> {
        self.upgrade().map(Cow::Owned).ok_or(ArcUpgradeError)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ArcUpgradeError;

impl core::fmt::Debug for ArcUpgradeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Cannot upgrade a dangling weak to an Rc")
    }
}
