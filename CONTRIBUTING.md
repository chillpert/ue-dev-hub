# Contributing

Bug reports, ideas and pull requests are welcome. For anything bigger than a fix, open an issue first
so we can agree on the approach.

## Building and testing

Needs Rust (the version in `rust-toolchain.toml` installs by itself through rustup), Node.js 22 and
the [Tauri prerequisites](https://tauri.app/start/prerequisites/).

```sh
npm install
npm run tauri dev                          # run the hub with hot reload
npm run build                              # type-check and build the UI

cd src-tauri
cargo test                                 # the Git tests use throwaway repositories
cargo clippy --all-targets -- -D warnings

cd ../addon-sdk
cargo test
cargo clippy --all-targets -- -D warnings
```

CI runs all of these on Windows and Linux for every pull request.

## Pull requests

- Branch from `main` and keep your branch up to date by rebasing onto it. No merge commits.
- Keep commits small and focused. Write messages like the ones in `git log`: a short imperative
  subject ("Show the engine version on Setup"), then a body that explains why if it isn't obvious.
- UI text is short and plain. Say what happens and what to do, without jargon.
- Add or update tests for behavior you change.

By contributing, you agree that your work is released under the [MIT License](LICENSE).
