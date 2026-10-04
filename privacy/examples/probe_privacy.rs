fn main() {
    let blocker = brows12_privacy::PrivacyBlocker::new();
    let urls = [
        "https://doubleclick.net/instream/ad_status.js",
        "https://securepubads.g.doubleclick.net/tag/js/gpt.js",
        "https://www.googletagservices.com/tag/js/gpt.js",
        "https://google-analytics.com/analytics.js",
        "https://www.google-analytics.com/analytics.js",
        "https://connect.facebook.net/en_US/fbevents.js",
        "https://static.chartbeat.com/js/chartbeat.js",
        "https://cdn.taboola.com/libtrc/loader.js",
    ];
    for u in urls {
        let v = blocker.check(u, "news.com", brows12_privacy::blocker::RequestKind::Script);
        let blocked = v.block.is_some();
        println!("{blocked} {u}");
    }
}
