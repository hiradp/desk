# Development guidance

`desk` is a Rust CLI that builds and extends a terminal workspace inside a
multiplexer. `README.md` documents the commands and the config; keep it in
sync with behaviour.

## Structure

- `src/backend.rs`: the multiplexer trait.
- `src/cli.rs`: command-line syntax only.
- `src/commands.rs`: what each command does, against the trait.
- `src/completion.rs`: shell completion of targets and sessions.
- `src/config.rs`: config loading, includes and validation.
- `src/error.rs`: error type and exit codes.
- `src/rex.rs`: the Rex backend, driving the `rex` CLI.

## Quality gates

```sh
just check
just test
```

CI runs the non-mutating equivalents:

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
```

Keep `rust-toolchain.toml` and `Cargo.lock` committed. Pin every GitHub
Actions `uses:` to a full commit SHA with a release comment.
