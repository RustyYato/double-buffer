#![allow(unused, clippy::let_unit_value)]

use super::SimpleAsyncStrategy as FlashStrategy;

use crate::{
    delay::DelayWriter,
    raw::{DoubleBufferData, Writer},
    strategy::flashmap::AsyncParkToken,
};

use pollster::test as async_test;

#[async_test]
async fn smoke() {
    let mut state = DoubleBufferData::new(0, 1, FlashStrategy::new());
    let mut writer = Writer::new(&mut state);

    let mut reader = writer.reader();

    let x = reader.read();
    assert_eq!(*x, *writer.split().read);

    // SAFETY: afinish_swap is polled to completion before split_mut/get_mut is called
    let mut swap = unsafe { writer.try_start_swap().unwrap() };

    // SAFETY: the swap is the latest swap
    assert!(!unsafe { writer.is_swap_finished(&mut swap) });

    assert_eq!(*x, *writer.split().write);

    drop(x);

    // SAFETY: the swap is the latest swap
    assert!(unsafe { writer.is_swap_finished(&mut swap) });

    // SAFETY: the swap is the latest swap
    unsafe { writer.afinish_swap(swap).await }
}

#[async_test]
async fn test_double_swap() {
    let mut state = DoubleBufferData::new(0, 1, FlashStrategy::new());
    let mut writer = Writer::new(&mut state);

    let mut reader = writer.reader();

    let x = reader.read();
    assert_eq!(*x, *writer.split().read);

    // SAFETY: swap is called, which ensures that finish_swap is called
    unsafe { writer.try_start_swap().unwrap() };
    assert_eq!(*x, *writer.split().write);
    // SAFETY: afinish_swap is called before split_mut or get_mut
    unsafe {
        let swap = writer.try_start_swap().unwrap();
        writer.afinish_swap(swap).await;
    }

    assert_eq!(*x, *writer.split().read);
}
