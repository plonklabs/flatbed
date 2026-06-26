# Contributing to flatbed

Thanks for your interest. flatbed is a small framework with a focused
surface — clarity beats cleverness, and `cargo fmt --all` is the universal
solvent.

## Development setup

```bash
git clone https://github.com/plonklabs/flatbed
cd flatbed
cargo build --workspace
cargo test --workspace
```

You'll need `flatc` (the FlatBuffer compiler) installed and on your `PATH`
at the version pinned in `.flatc-version`. `flatbed_build`'s codegen calls
it twice per `.fbs` (once for the standard Rust accessors, once for the
binary reflection schema flatbed reads to emit its ergonomic layer).

Verify the committed FlatBuffer-generated code matches the schemas:

```bash
bash scripts/check-generated.sh
```

## Pull requests

- Keep changes focused. A `cargo fmt --all` + `cargo clippy --workspace
  --all-targets --all-features -- -D warnings` pass beats discussion of
  style nits.
- Run `cargo test --workspace` before pushing.
- Comments should describe the WHY, not the WHAT. Forward-looking
  phrasing ("future X will…", "TODO when Y") rots — describe the current
  invariant instead.
- New public APIs need doc comments and reasonably representative
  examples.

## License of contributions

Unless you explicitly state otherwise, any contribution you intentionally
submit for inclusion in flatbed shall be dual-licensed under the MIT and
Apache-2.0 licenses (see [LICENSE-MIT](LICENSE-MIT) and
[LICENSE-APACHE](LICENSE-APACHE)) without any additional terms or
conditions. This is the Apache-2.0 § 5 default; the explicit notice here
exists so contributors see it without reading the full license text.
