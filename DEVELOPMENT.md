# Local development notes

## Browsing the API documentation

Generate the rustdoc output from the repository root and open it in a browser.
No extra tools are needed.

```sh
cargo doc --no-deps --open
```

Without `--open`, the output is written to `target/doc/lz4opt/index.html`.

Without a browser, read the doc comments (`///`) in the source directly.

## Checks

Run the same checks as CI locally.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --release
```

The interoperability tests against the official `lz4` command are skipped, with a message giving the reason, when `lz4` is not on the PATH.
To run them, install it with `brew install lz4` (macOS) or `apt-get install lz4` (Linux).
