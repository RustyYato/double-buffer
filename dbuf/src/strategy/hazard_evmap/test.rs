#![cfg(feature = "std")]
#![allow(unused)]

use super::HazardEvMapStrategy;

use crate::{
    delay::DelayWriter,
    raw::{DoubleBufferData, Writer},
    strategy::flash_park_token::AsyncParkToken,
};

use pollster::test as async_test;

#[test]
fn smoke() {
    let mut state = DoubleBufferData::new(0, 1, HazardEvMapStrategy::new_blocking());
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
