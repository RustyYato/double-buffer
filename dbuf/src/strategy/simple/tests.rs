#![allow(unused, clippy::let_unit_value)]

use super::SimpleStrategy as FlashStrategy;

use crate::{
    delay::DelayWriter,
    raw::{DoubleBufferData, Writer},
};

use pollster::test as async_test;

#[async_test]
async fn smoke() {
    let mut state = DoubleBufferData::new(0, 1, FlashStrategy::new());
    let mut writer = Writer::new(&mut state);

    let mut reader = writer.reader();

    let x = reader.read();
    assert_eq!(*x, *writer.split().read);

    drop(x);

    // SAFETY: afinish_swap is polled to completion before split_mut/get_mut is called
    let mut swap = unsafe { writer.try_start_swap().unwrap() };

    // SAFETY: the swap is the latest swap
    assert!(unsafe { writer.is_swap_finished(&mut swap) });

    // SAFETY: the swap is the latest swap
    unsafe { writer.afinish_swap(swap).await }
}
