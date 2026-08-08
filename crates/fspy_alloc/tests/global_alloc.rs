//! Installs [`FspyAlloc`] as this test binary's global allocator, so every
//! allocation — including the test harness's own — exercises the allocator
//! end-to-end over real memory mappings.
#![cfg(all(unix, not(miri)))] // Miri covers the pool core via its mockable backend instead.

use std::{collections::BTreeMap, sync::mpsc, thread};

use fspy_alloc::FspyAlloc;

#[global_allocator]
static GLOBAL: FspyAlloc = FspyAlloc::new();

#[test]
fn collections_round_trip() {
    let mut map = BTreeMap::new();
    for i in 0..1000_u32 {
        let len = usize::try_from(i % 300).unwrap();
        map.insert(i, vec![0xAB_u8; len]);
    }
    assert_eq!(map.len(), 1000);
    for (i, bytes) in &map {
        assert_eq!(bytes.len(), usize::try_from(i % 300).unwrap());
        assert!(bytes.iter().all(|byte| *byte == 0xAB));
    }
}

#[test]
fn vec_growth_reallocs_preserve_contents() {
    let mut bytes = Vec::new();
    for i in 0..1_000_000_usize {
        bytes.push(u8::try_from(i % 251).unwrap());
    }
    for (i, byte) in bytes.iter().enumerate() {
        assert_eq!(usize::from(*byte), i % 251);
    }
}

#[test]
fn large_and_zeroed_allocations() {
    // Direct-mapped (beyond the largest size class), via the zeroing path.
    let large = vec![0_u8; 5 * 1024 * 1024];
    assert!(large.iter().all(|byte| *byte == 0));
    // And through the plain path.
    let boxed: Box<[u8]> = vec![7_u8; 300 * 1024].into_boxed_slice();
    assert!(boxed.iter().all(|byte| *byte == 7));
}

#[test]
fn many_small_allocations_span_slabs() {
    // More live 16-byte-class blocks than one slab holds, forcing the pool
    // through several slab installations (and the holding Vec through the
    // direct-mapped path as it grows).
    let boxes: Vec<Box<[u8; 8]>> = (0..120_000).map(|_| Box::new([0xEE_u8; 8])).collect();
    assert!(boxes.iter().all(|block| block.iter().all(|byte| *byte == 0xEE)));
}

#[test]
fn threaded_producers_and_consumers() {
    let threads = 8;
    let per_thread = 5_000_usize;
    let (sender, receiver) = mpsc::channel::<Vec<u8>>();
    thread::scope(|scope| {
        for t in 0..threads {
            let sender = sender.clone();
            scope.spawn(move || {
                for i in 0..per_thread {
                    // Vary sizes across classes; blocks are freed (and often
                    // allocated) on the consumer thread below.
                    let len = 1 + (i * 37 + t * 101) % 5000;
                    sender.send(vec![u8::try_from(t % 251).unwrap(); len]).unwrap();
                }
            });
        }
        drop(sender);
        let mut message_count = 0_usize;
        for bytes in receiver {
            message_count += 1;
            assert!(!bytes.is_empty());
            let first = bytes[0];
            assert!(bytes.iter().all(|byte| *byte == first));
        }
        assert_eq!(message_count, threads * per_thread);
    });
}
