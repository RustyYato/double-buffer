#![allow(unused)]

use super::FlashStrategy;

use crate::raw::{DoubleBufferData, Writer};

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
