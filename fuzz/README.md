# Fuzzing

Targets for `cargo-fuzz` (separate workspace; requires nightly):

```sh
cd fuzz
cargo +nightly fuzz run html_parse -- -max_len=65536
cargo +nightly fuzz run css_parse
cargo +nightly fuzz run doh_wire_parse
cargo +nightly fuzz run cookie_parse
```

The network parser targets the hand-rolled DNS wire-format reader
(name-compression loops and length fields are classic memory-safety traps).
`cookie_parse` exercises Set-Cookie attribute handling.
