fn main() {
    let blocker = brows12_privacy::PrivacyBlocker::new();
    let cases = [
        ("https://example.com/get?utm_source=brows12test&id=42", "127.0.0.1", "image"),
        ("https://example.com/get?utm_source=brows12test&id=42", "example.com", "document"),
        ("https://site.com/page?utm_medium=cpc&x=1", "site.com", "main_frame"),
    ];
    for (u, src, kind) in cases {
        let v = blocker.check(u, src, brows12_privacy::blocker::RequestKind::Image);
        // Note: kind mapping not critical for removeparam
        println!(
            "{u} [{src}/{kind}] blocked={} rewritten={:?}",
            v.block.is_some(),
            v.rewritten_url
        );
    }
    let blocker2 = brows12_privacy::PrivacyBlocker::from_filters("*$removeparam=utm_source\n");
    let v = blocker2.check(
        "https://example.com/get?utm_source=x&id=42",
        "example.com",
        brows12_privacy::blocker::RequestKind::Image,
    );
    println!("minimal: blocked={} rewritten={:?}", v.block.is_some(), v.rewritten_url);
}
