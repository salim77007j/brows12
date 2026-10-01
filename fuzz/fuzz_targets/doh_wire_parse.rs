#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // The DoH response parser must survive arbitrary wire bytes.
    let _ = brows12_net::dns::parse_response(data, "example.com", 1);
});
