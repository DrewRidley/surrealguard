# surrealql-analyzer

The `surrealql-analyzer` CLI: a static analyzer and type-inference engine for
SurrealQL. It checks your queries against your `.surql` schema and generates
TypeScript types for them.

The CLI itself is written in Rust. This npm package is a launcher: on first run
it detects your platform, downloads the matching prebuilt binary from the
[GitHub Release](https://github.com/surrealdb/analyzer/releases) for **this
package's version**, caches it inside the package directory (`binaries/<target>/`),
and execs it. Later runs skip straight to the cached binary. Because the version
pins the release tag, `npx surrealql-analyzer@0.4.0` always gets the 0.4.0 binary — the
launcher and the binary can never drift apart.

## Use

```sh
npx surrealql-analyzer --help
```

Or add it to a project, which is what you usually want so that CI and every
developer run the same version:

```sh
npm i -D surrealql-analyzer
npx surrealql-analyzer init      # writes a commented surrealql-analyzer.toml
npx surrealql-analyzer check     # analyze; non-zero exit if errors remain
npx surrealql-analyzer generate  # emit the typed TypeScript client
```

`check` also takes `--json` for machine-readable diagnostics, and `generate`
takes `--out <path>`. Run `surrealql-analyzer <command> --help` for the flags your
installed version has.

## Requirements

Node >= 16, plus `tar` (present on macOS and Linux) or PowerShell's
`Expand-Archive` on Windows — used once, to unpack the downloaded archive.
Nothing else is installed.

## Supported platforms

| platform | arch  | target triple             |
| -------- | ----- | ------------------------- |
| macOS    | arm64 | aarch64-apple-darwin      |
| macOS    | x64   | x86_64-apple-darwin       |
| Linux    | x64   | x86_64-unknown-linux-gnu  |
| Linux    | arm64 | aarch64-unknown-linux-gnu |
| Windows  | x64   | x86_64-pc-windows-msvc    |

On anything else the launcher exits with a message naming your platform; build
from source instead.

## Without npm

The same prebuilt binaries, via
[`cargo binstall`](https://github.com/cargo-bins/cargo-binstall):

```sh
cargo binstall surrealql-analyzer
```

Or from source:

```sh
cargo install surrealql-analyzer
```

## Docs

Full documentation, including the TypeScript packages this generates types for,
is at [github.com/surrealdb/analyzer](https://github.com/surrealdb/analyzer).

## License

MIT OR Apache-2.0
