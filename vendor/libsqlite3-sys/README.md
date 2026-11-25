# Bundled SQLite

This package retains the libsqlite3-sys 0.30.1 Rust bindings used by rusqlite 0.32.
Its SQLite amalgamation and public header are from upstream SQLite 3.54.1.
The C source was obtained from rusqlite's upstream mirror, Git blob
`ac7549011639e3f1df0b6ef6faf85760a2d2292e`. Its SHA3-256 matches the
official SQLite release record:

`e2b869d3003b33c942b7dd7063604fbc53553e1b66609570df39b9b7b558ee81`

See https://sqlite.org/changes.html and https://sqlite.org/wal.html#the_wal_reset_bug.
