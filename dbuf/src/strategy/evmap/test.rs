#![allow(unused)]

use super::EvMapStrategy;

use crate::{
    delay::DelayWriter,
    raw::{DoubleBufferData, Writer},
    strategy::{atomic::park_token::ThreadParkToken, flash_park_token::AsyncParkToken},
};

use pollster::test as async_test;

#[test]
fn smoke() {
    let mut state = DoubleBufferData::new(0, 1, EvMapStrategy::<ThreadParkToken>::new());
    let mut writer = Writer::new(&mut state);

    let mut reader = writer.reader();

    let x = reader.read();
    assert_eq!(*x, *writer.split().read);

    // SAFETY: finish_swap is called before split_mut/get_mut is called
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
fn test_issue_1() {
    let mut data = DoubleBufferData::new(1, 2, EvMapStrategy::<ThreadParkToken>::new());

    let writer = Writer::new(&mut data);
    let mut writer = DelayWriter::from_writer(writer);

    let mut reader1 = writer.reader();
    assert_eq!(*reader1.read(), 1);

    writer.start_swap();
    writer.finish_swap();

    let mut reader2 = writer.reader();
    let mut reader3 = reader1.clone();
    assert_eq!(*reader1.read(), 2);
    assert_eq!(*reader2.read(), 2);
    assert_eq!(*reader3.read(), 2);

    let guard = reader2.read();

    assert_eq!(*guard, 2);

    *writer.get_writer_mut().unwrap().get_mut() = 3;

    assert_eq!(*guard, 2);
    drop(guard);

    assert_eq!(*reader1.read(), 2);
    assert_eq!(*reader2.read(), 2);
    assert_eq!(*reader3.read(), 2);
}
