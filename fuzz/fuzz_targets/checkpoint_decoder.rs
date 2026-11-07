#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Container parsing is file-oriented. Payload decoding covers the bounded
    // postcard section independently; container corruption is covered by unit tests.
    let _ = monstermemory_persistence::decode_table::<Vec<(u64, Vec<u8>)>>(data);
});

