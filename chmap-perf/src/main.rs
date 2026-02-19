use std::{
    collections::{HashMap, hash_map::Entry},
    convert::Infallible,
    fs::read_to_string,
    hint::black_box,
    path::PathBuf,
    sync::{Mutex, RwLock},
    time::Instant,
};

use color_eyre::owo_colors::colors::xterm::ConiferGreen;
use dbuf::interface::{BlockingStrategy, Strategy};
use rand::seq::{IndexedRandom, IteratorRandom};
use tracing::{debug, info};

#[derive(clap::Parser)]
struct Args {
    config: PathBuf,
    #[clap(flatten)]
    verbose: clap_verbosity_flag::Verbosity<clap_verbosity_flag::InfoLevel>,
    // #[clap(long, short)]
    // reads: u32,
    // #[clap(long, short)]
    // writes: u32,
    // #[clap(long, short)]
    // sync: u32,
    // #[clap(long, short)]
    // threads: u32,
    // #[clap(long, short)]
    // ops: u32,
}

#[derive(serde::Deserialize, Clone)]
pub struct ConfigEntry {
    name: String,
    #[serde(rename = "num-threads")]
    num_threads: u32,
    ops: u32,
    odds: ConfigEntryOdds,
}

#[derive(serde::Deserialize, Clone, Copy)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ConfigEntryOdds {
    #[serde(default)]
    reads: u32,
    #[serde(default)]
    reads_failed: u32,
    #[serde(default)]
    inserts: u32,
    #[serde(default)]
    inserts_new: u32,
    #[serde(default)]
    deletes: u32,
    #[serde(default)]
    deletes_failed: u32,
    #[serde(default)]
    syncs: u32,
}

fn main() -> eyre::Result<()> {
    color_eyre::install().unwrap();

    let args: Args = clap::Parser::parse();

    tracing_subscriber::fmt()
        .with_max_level(args.verbose.tracing_level_filter())
        .init();

    let config = std::fs::read(args.config)?;
    let config: Vec<ConfigEntry> = serde_json::from_slice(&config)?;

    let mut thread_ops = Vec::new();
    let total_ops;

    {
        let mut map = HashMap::<u32, u32>::new();

        total_ops = config
            .iter()
            .try_fold(0, |acc, entry| entry.ops.checked_add(acc))
            .expect("Tried to add too many ops");

        if total_ops == 0 {
            tracing::error!("Tried to create a perf test with no operations");
            return Ok(());
        }

        let thread_range_len = u32::MAX / total_ops;
        let mut start = 0;

        for entry in &config {
            if entry.ops == 0 {
                continue;
            }

            let mut ops = Vec::<MapOps>::with_capacity(entry.ops as usize);

            let ConfigEntryOdds {
                reads,
                reads_failed,
                inserts,
                inserts_new,
                deletes,
                deletes_failed,
                syncs,
            } = entry.odds;

            let use_writer = entry.ops != 0
                && (inserts != 0
                    || inserts_new != 0
                    || deletes != 0
                    || deletes_failed != 0
                    || syncs != 0);

            let Some(total) = [
                reads,
                reads_failed,
                inserts,
                inserts_new,
                deletes,
                deletes_failed,
                syncs,
            ]
            .iter()
            .try_fold(0, |acc, &entry| entry.checked_add(acc)) else {
                eyre::bail!("Odds on entry overflowed u32")
            };

            let mut running_total = 0;
            let mut to_percent = |odds: u32| {
                running_total += odds;
                (running_total as f64 / total as f64) as f32
            };

            let reads = to_percent(reads);
            let reads_failed = to_percent(reads_failed);
            let inserts = to_percent(inserts);
            let inserts_new = to_percent(inserts_new);
            let deletes = to_percent(deletes);
            let deletes_failed = to_percent(deletes_failed);
            let syncs = to_percent(syncs);

            assert_eq!(syncs, 1.0);
            let valid_range = start..start + thread_range_len / 2;
            let invalid_range = start + thread_range_len / 2..start + thread_range_len;
            let valid_range = 0..u32::MAX / 2;
            let valid_range = u32::MAX / 2..u32::MAX;
            start += thread_range_len;

            while ops.len() < ops.capacity() {
                let r: f32 = rand::random();
                ops.push(if r < reads {
                    let Some(&key) = map.keys().choose(&mut rand::rng()) else {
                        continue;
                    };
                    let expected_val = map[&key];
                    MapOps::Read { key, expected_val }
                } else if r < reads_failed {
                    let key = rand::random_range(invalid_range.clone());
                    MapOps::ReadNotExists { key }
                } else if r < inserts {
                    let key = rand::random_range(valid_range.clone());
                    let val = rand::random();
                    map.insert(key, val);
                    MapOps::Insert { key, val }
                } else if r < inserts_new {
                    if map.len() == valid_range.len() {
                        continue;
                    }

                    loop {
                        let key = rand::random_range(valid_range.clone());
                        let val = rand::random();
                        match map.entry(key) {
                            Entry::Occupied(_) => continue,
                            Entry::Vacant(vacant_entry) => {
                                vacant_entry.insert(val);
                                break MapOps::Insert { key, val };
                            }
                        }
                    }
                } else if r < deletes {
                    let Some(&key) = map.keys().choose(&mut rand::rng()) else {
                        continue;
                    };
                    let expected_val = map.remove(&key).unwrap();
                    MapOps::Remove { key, expected_val }
                } else if r < deletes_failed {
                    let key = rand::random_range(invalid_range.clone());
                    MapOps::RemoveNotExists { key }
                } else {
                    MapOps::Sync
                });
            }

            thread_ops.extend(std::iter::repeat_n(
                (use_writer, entry.name.as_str(), ops),
                entry.num_threads as usize,
            ));
        }
    }

    let thread_ops = thread_ops.as_slice();

    run_chmap::<
        dbuf::strategy::flashmap::FlashStrategy<
            dbuf::strategy::flash_park_token::AdaptiveParkToken,
        >,
    >(thread_ops);

    run_chmap::<
        dbuf::strategy::evmap::EvMapStrategy<dbuf::strategy::atomic::park_token::ThreadParkToken>,
    >(thread_ops);

    run_chmap::<
        dbuf::strategy::atomic::AtomicStrategy<dbuf::strategy::atomic::park_token::ThreadParkToken>,
    >(thread_ops);

    run_evmap(thread_ops);
    run_flashmap(thread_ops);
    run_dashmap(thread_ops);

    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum MapOps {
    Sync,
    Read { key: u32, expected_val: u32 },
    ReadNotExists { key: u32 },
    Remove { key: u32, expected_val: u32 },
    RemoveNotExists { key: u32 },
    Insert { key: u32, val: u32 },
}

fn run_chmap<S>(thread_ops: &[(bool, &str, Vec<MapOps>)])
where
    S: Send + Sync + BlockingStrategy + Default,
    S::WriterId: Send + Sync,
    S::Swap: Send + Sync,
    S::ReaderId: Send,
{
    let start = Instant::now();
    if tracing::enabled!(tracing::Level::DEBUG) {
        println!("{:=>160}", "");
    }
    debug!("{}", core::any::type_name::<S>());
    let writer = chmap::Writer::<_, _, chmap::DefaultHasher, S>::default();
    let reader = writer.reader();
    let writer = &RwLock::new(writer);

    std::thread::scope(|s| {
        for &(use_writer, name, ref thread) in thread_ops {
            let thread = thread.as_slice();

            if use_writer {
                s.spawn(move || {
                    for op in thread {
                        match *op {
                            MapOps::Sync => {
                                let _ = writer.write().unwrap().try_publish();
                            }
                            MapOps::Read {
                                key,
                                expected_val: _,
                            } => {
                                black_box(writer.read().unwrap().get(&key));
                            }
                            MapOps::ReadNotExists { key } => {
                                black_box(writer.read().unwrap().get(&key));
                            }
                            MapOps::Remove {
                                key,
                                expected_val: _,
                            } => {
                                writer.write().unwrap().remove(key);
                            }
                            MapOps::RemoveNotExists { key } => {
                                writer.write().unwrap().remove(key);
                            }
                            MapOps::Insert { key, val } => {
                                writer.write().unwrap().insert(key, val);
                            }
                        }
                    }
                    debug!("THREAD COMPLETE* {name}")
                });
            } else {
                let mut reader = reader.clone();
                s.spawn(move || {
                    for op in thread {
                        match *op {
                            MapOps::Read {
                                key,
                                expected_val: val,
                            } => {
                                let map = black_box(reader.load());
                                black_box(map.get(&key));
                            }
                            MapOps::ReadNotExists { key } => {
                                let map = black_box(reader.load());
                                assert_eq!(map.get(&key), None);
                            }
                            MapOps::Sync
                            | MapOps::Remove {
                                key: _,
                                expected_val: _,
                            }
                            | MapOps::RemoveNotExists { key: _ }
                            | MapOps::Insert { key: _, val: _ } => unreachable!(),
                        }
                    }
                    debug!("THREAD COMPLETE {name}")
                });
            }
        }
    });
    info!(time = ?start.elapsed(), "{}", core::any::type_name::<S>());
}

fn run_flashmap(thread_ops: &[(bool, &str, Vec<MapOps>)]) {
    let start = Instant::now();
    if tracing::enabled!(tracing::Level::DEBUG) {
        println!("{:=>160}", "");
    }
    debug!("flashmap");

    let (writer, reader) = unsafe { flashmap::with_hasher(chmap::DefaultHasher::new()) };
    let writer = &Mutex::new(writer);

    std::thread::scope(|s| {
        for &(use_writer, name, ref thread) in thread_ops {
            let thread = thread.as_slice();

            if use_writer {
                let reader = reader.clone();
                s.spawn(move || {
                    for op in thread {
                        match *op {
                            MapOps::Sync => {
                                writer.lock().unwrap().synchronize();
                            }
                            MapOps::Read {
                                key,
                                expected_val: _,
                            } => {
                                black_box(reader.guard().get(&key));
                            }
                            MapOps::ReadNotExists { key } => {
                                black_box(reader.guard().get(&key));
                            }
                            MapOps::Remove {
                                key,
                                expected_val: _,
                            } => {
                                writer.lock().unwrap().guard().remove(key);
                            }
                            MapOps::RemoveNotExists { key } => {
                                writer.lock().unwrap().guard().remove(key);
                            }
                            MapOps::Insert { key, val } => {
                                writer.lock().unwrap().guard().insert(key, val);
                            }
                        }
                    }
                    debug!("THREAD COMPLETE* {name}")
                });
            } else {
                let reader = reader.clone();
                s.spawn(move || {
                    for op in thread {
                        match *op {
                            MapOps::Read {
                                key,
                                expected_val: val,
                            } => {
                                black_box(reader.guard().get(&key));
                            }
                            MapOps::ReadNotExists { key } => {
                                assert_eq!(reader.guard().get(&key), None);
                            }
                            MapOps::Sync
                            | MapOps::Remove {
                                key: _,
                                expected_val: _,
                            }
                            | MapOps::RemoveNotExists { key: _ }
                            | MapOps::Insert { key: _, val: _ } => unreachable!(),
                        }
                    }
                    debug!("THREAD COMPLETE {name}")
                });
            }
        }
    });
    info!(time = ?start.elapsed(), "flashmap");
}

fn run_evmap(thread_ops: &[(bool, &str, Vec<MapOps>)]) {
    let start = Instant::now();
    if tracing::enabled!(tracing::Level::DEBUG) {
        println!("{:=>160}", "");
    }
    debug!("evmap");
    let (writer, reader) = unsafe { evmap::with_hasher((), chmap::DefaultHasher::new()) };
    let writer = &Mutex::new(writer);

    std::thread::scope(|s| {
        for &(use_writer, name, ref thread) in thread_ops {
            let thread = thread.as_slice();

            if use_writer {
                let reader = reader.clone();
                s.spawn(move || {
                    for op in thread {
                        match *op {
                            MapOps::Sync => {
                                writer.lock().unwrap().publish();
                            }
                            MapOps::Read {
                                key,
                                expected_val: _,
                            } => {
                                black_box(reader.get(&key));
                            }
                            MapOps::ReadNotExists { key } => {
                                black_box(reader.get(&key));
                            }
                            MapOps::Remove {
                                key,
                                expected_val: _,
                            } => {
                                writer.lock().unwrap().remove_entry(key);
                            }
                            MapOps::RemoveNotExists { key } => {
                                writer.lock().unwrap().remove_entry(key);
                            }
                            MapOps::Insert { key, val } => {
                                writer.lock().unwrap().insert(key, val);
                            }
                        }
                    }
                    debug!("THREAD COMPLETE* {name}")
                });
            } else {
                let reader = reader.clone();
                s.spawn(move || {
                    for op in thread {
                        match *op {
                            MapOps::Read {
                                key,
                                expected_val: val,
                            } => {
                                black_box(reader.get(&key));
                            }
                            MapOps::ReadNotExists { key } => {
                                assert!(reader.get(&key).is_none());
                            }
                            MapOps::Sync
                            | MapOps::Remove {
                                key: _,
                                expected_val: _,
                            }
                            | MapOps::RemoveNotExists { key: _ }
                            | MapOps::Insert { key: _, val: _ } => unreachable!(),
                        }
                    }
                    debug!("THREAD COMPLETE {name}")
                });
            }
        }
    });
    info!(time = ?start.elapsed(), "evmap");
}

fn run_dashmap(thread_ops: &[(bool, &str, Vec<MapOps>)]) {
    let start = Instant::now();
    if tracing::enabled!(tracing::Level::DEBUG) {
        println!("{:=>160}", "");
    }
    debug!("dashmap");
    let map = &dashmap::DashMap::with_hasher(chmap::DefaultHasher::new());

    std::thread::scope(|s| {
        for &(use_writer, name, ref thread) in thread_ops {
            let thread = thread.as_slice();

            if use_writer {
                s.spawn(move || {
                    for op in thread {
                        match *op {
                            MapOps::Sync => {}
                            MapOps::Read {
                                key,
                                expected_val: _,
                            } => {
                                black_box(map.get(&key));
                            }
                            MapOps::ReadNotExists { key } => {
                                black_box(map.get(&key));
                            }
                            MapOps::Remove {
                                key,
                                expected_val: _,
                            } => {
                                map.remove(&key);
                            }
                            MapOps::RemoveNotExists { key } => {
                                map.remove(&key);
                            }
                            MapOps::Insert { key, val } => {
                                map.insert(key, val);
                            }
                        }
                    }
                    debug!("THREAD COMPLETE* {name}")
                });
            } else {
                s.spawn(move || {
                    for op in thread {
                        match *op {
                            MapOps::Read {
                                key,
                                expected_val: val,
                            } => {
                                black_box(map.get(&key));
                            }
                            MapOps::ReadNotExists { key } => {
                                assert!(map.get(&key).is_none());
                            }
                            MapOps::Sync
                            | MapOps::Remove {
                                key: _,
                                expected_val: _,
                            }
                            | MapOps::RemoveNotExists { key: _ }
                            | MapOps::Insert { key: _, val: _ } => unreachable!(),
                        }
                    }
                    debug!("THREAD COMPLETE {name}")
                });
            }
        }
    });
    info!(time = ?start.elapsed(), "dashmap");
}
