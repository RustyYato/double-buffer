#![allow(unused)]

use super::FlashStrategy;

use crate::raw::{DoubleBufferData, Writer};

use pollster::test as async_test;

#[test]
fn smoke() {
    let mut state = DoubleBufferData::new(0, 1, FlashStrategy::new());
    let mut writer = Writer::new(&mut state);

    let mut reader = writer.reader();

    let x = reader.read();
    assert_eq!(*x, *writer.split().read);

    let mut swap = unsafe { writer.try_start_swap().unwrap() };

    assert!(!unsafe { writer.is_swap_finished(&mut swap) });

    assert_eq!(*x, *writer.split().write);

    drop(x);

    assert!(unsafe { writer.is_swap_finished(&mut swap) });

    unsafe { writer.finish_swap(swap) }
}

#[test]
fn test_double_swap() {
    let mut state = DoubleBufferData::new(0, 1, FlashStrategy::new());
    let mut writer = Writer::new(&mut state);

    let mut reader = writer.reader();

    let x = reader.read();
    assert_eq!(*x, *writer.split().read);

    unsafe { writer.try_start_swap().unwrap() };
    assert_eq!(*x, *writer.split().write);
    writer.swap();

    assert_eq!(*x, *writer.split().read);
}

#[async_test]
async fn test_async() {
    let mut state = DoubleBufferData::new(0, 1, FlashStrategy::with_park_token());
    let mut writer = Writer::new(&mut state);

    let mut reader = writer.reader();

    let x = reader.read();
    assert_eq!(*x, *writer.split().read);

    unsafe { writer.try_start_swap().unwrap() };
    assert_eq!(*x, *writer.split().write);
    writer.aswap().await;

    assert_eq!(*x, *writer.split().read);
}
