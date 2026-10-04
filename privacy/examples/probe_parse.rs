use adblock::filters::network::NetworkFilter;
use adblock::lists::ParseOptions;
fn main() {
    for line in [
        "*$removeparam=utm_source",
        "$removeparam=utm_source",
        "*$removeparam=utm_source, image",
        "||example.com^$removeparam=utm_source",
        "*$removeparam=/^utm_/",
    ] {
        match NetworkFilter::parse(line, true, ParseOptions::default()) {
            Ok(f) => println!("OK   {line} -> modifier={:?}", f.modifier_option),
            Err(e) => println!("ERR  {line} -> {e}"),
        }
    }
}
