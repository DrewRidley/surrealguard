# surrealguard

Static analyzer and type-inference engine for SurrealQL.

This npm package is a thin launcher for the `surrealguard` CLI (written in Rust).
The first time you run it, it downloads the prebuilt binary that matches your
platform from the [GitHub Releases](https://github.com/DrewRidley/surrealguard/releases)
and caches it inside the package directory; subsequent runs exec the cached binary
directly.

## Usage

```sh
npx surrealguard --help
```

Or install it:

```sh
npm install -g surrealguard
surrealguard --help
```

## Supported platforms

| platform | arch  | target triple               |
| -------- | ----- | --------------------------- |
| macOS    | arm64 | aarch64-apple-darwin        |
| macOS    | x64   | x86_64-apple-darwin         |
| Linux    | x64   | x86_64-unknown-linux-gnu    |
| Linux    | arm64 | aarch64-unknown-linux-gnu   |
| Windows  | x64   | x86_64-pc-windows-msvc      |

## Alternative install methods

Prebuilt binaries are also available via [`cargo binstall`](https://github.com/cargo-bins/cargo-binstall):

```sh
cargo binstall surrealguard
```

Or build from source:

```sh
cargo install surrealguard
```

## License

MIT OR Apache-2.0
