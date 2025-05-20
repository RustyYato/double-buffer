#![allow(unused, clippy::let_unit_value)]

use super::SimpleStrategy;

use crate::{
    delay::DelayWriter,
    raw::{DoubleBufferData, Writer},
};

use pollster::test as async_test;

#[async_test]
async fn smoke() {
    let mut state = DoubleBufferData::new(0, 1, SimpleStrategy::new());
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
    unsafe { writer.afinish_swap(&mut { swap }).await };
}

#[test]
fn test_issue_1() {
    let mut data = DoubleBufferData::new(1, 2, SimpleStrategy::new());

    let writer = Writer::new(&mut data);
    let mut writer = DelayWriter::from_writer(writer);

    let mut reader1 = writer.reader();
    assert_eq!(*reader1.read(), 1);

    writer.start_swap();
    writer.finish_swap();

    let mut reader2 = writer.reader();
    assert_eq!(*reader1.read(), 2);
    assert_eq!(*reader2.read(), 2);

    let guard = reader2.read();

    assert_eq!(*guard, 2);

    *writer.get_writer_mut().unwrap().get_mut() = 3;

    assert_eq!(*guard, 2);
    drop(guard);

    assert_eq!(*reader1.read(), 2);
    assert_eq!(*reader2.read(), 2);
}
