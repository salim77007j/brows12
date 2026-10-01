#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let mut jar = brows12_storage::CookieJar::new();
        let url = url::Url::parse("https://fuzz.test/").unwrap();
        jar.set_from_header(&url, s, "fuzz.test");
        let _ = jar.header_for_url(&url, "fuzz.test", false);
    }
});
