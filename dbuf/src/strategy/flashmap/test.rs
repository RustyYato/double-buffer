#![allow(unused)]

use super::FlashStrategy;

use crate::{
    delay::DelayWriter,
    raw::{DoubleBufferData, Writer},
    strategy::park_token::AsyncParkToken,
};

use pollster::test as async_test;

#[test]
fn smoke() {
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
    unsafe { writer.finish_swap(swap) }
}

#[test]
fn test_double_swap() {
    let mut state = DoubleBufferData::new(0, 1, FlashStrategy::new());
    let mut writer = Writer::new(&mut state);

    let mut reader = writer.reader();

    let x = reader.read();
    assert_eq!(*x, *writer.split().read);

    // SAFETY: swap is called, which ensures that finish_swap is called
    unsafe { writer.try_start_swap().unwrap() };
    assert_eq!(*x, *writer.split().write);
    writer.swap();

    assert_eq!(*x, *writer.split().read);
}

#[async_test]
async fn test_async() {
    let mut state = DoubleBufferData::new(0, 1, FlashStrategy::new_async());
    let mut writer = Writer::new(&mut state);

    let mut reader = writer.reader();

    let x = reader.read();
    assert_eq!(*x, *writer.split().read);

    // SAFETY: afinish_swap is polled to completion before split_mut/get_mut is called
    unsafe { writer.try_start_swap().unwrap() };
    assert_eq!(*x, *writer.split().write);
    // SAFETY: afinish_swap is polled to completion before split_mut/get_mut is called
    // the swap passed to afinish_swap is the latest swap
    unsafe {
        let swap = writer.try_start_swap().unwrap();
        writer.afinish_swap(&mut { swap }).await;
    }

    assert_eq!(*x, *writer.split().read);
}
