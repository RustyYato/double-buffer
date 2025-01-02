#[test]
#[cfg(loom)]
fn loom() {
    loom::model(|| {
        dbg!();
        let x = crate::raw::Writer::new(rc_box::RcBox::new(crate::raw::DoubleBufferData::new(
            loom::cell::UnsafeCell::new(0),
            loom::cell::UnsafeCell::new(0),
            AtomicStrategy::new(),
        )));

        let a = loom::thread::spawn({
            let mut x = x.reader();
            move || {
                {
                    let x = x.read();
                    x.with(|_| loom::thread::yield_now());
                }
                x
            }
        });

        let b = loom::thread::spawn(move || {
            let mut x = crate::delay::DelayWriter::from_writer(x);

            match x.get_writer_mut() {
                None => (),
                Some(x) => {
                    x.get().with_mut(|_| loom::thread::yield_now());
                }
            }

            x
        });

        let _a = a.join().unwrap();
        let _b = b.join().unwrap();
    });
}
