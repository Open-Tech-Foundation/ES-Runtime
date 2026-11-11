# hpack-test-case (subset)

Header sequences encoded by independent HPACK implementations, with the header
lists they decode to, from [hpack-test-case](https://github.com/http2jp/hpack-test-case)
at the revision in `REVISION`, under its MIT licence (`LICENSE`). Read by
`crates/http/tests/hpack_test_case.rs`.

Vendored: `story_00`–`story_07` of four encoders — nghttp2, Go's hpack,
Python's hpack, and nghttp2 with changing table sizes. The whole corpus is about
70 MB; `scripts/fetch-hpack-test-case.sh` fetches all of it into the ignored
`hpack-test-case-full/` beside this directory, and the same test then runs
every story of every encoder too.
