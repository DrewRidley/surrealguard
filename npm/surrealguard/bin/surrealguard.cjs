#!/usr/bin/env node
"use strict";

// Download-on-first-run launcher for the `surrealguard` CLI (wasm-pack / esbuild
// fallback pattern). On first invocation this detects the host platform + arch,
// maps it to a Rust target triple, downloads the matching prebuilt binary from
// the GitHub Release for this package's version, caches it under the package
// directory, then execs it forwarding argv and the exit code.
//
// ASSET NAMING SCHEME (must stay identical to crates/cli Cargo.toml
// [package.metadata.binstall] and .github/workflows/release.yml):
//
//   tag:     v{version}
//   archive: surrealguard-{version}-{target}.tar.gz   (unix)
//            surrealguard-{version}-{target}.zip       (windows)
//   binary:  surrealguard | surrealguard.exe           (at archive root)
//   url:     https://github.com/DrewRidley/surrealguard/releases/download/
//              v{version}/surrealguard-{version}-{target}.{ext}

const fs = require("fs");
const os = require("os");
const path = require("path");
const https = require("https");
const { execFileSync, spawnSync } = require("child_process");

const pkg = require(path.join(__dirname, "..", "package.json"));
const VERSION = pkg.version;
const REPO = "DrewRidley/surrealguard";
const RELEASES_PAGE = `https://github.com/${REPO}/releases`;

// process.platform + process.arch  ->  Rust target triple
const TARGETS = {
  "darwin-arm64": "aarch64-apple-darwin",
  "darwin-x64": "x86_64-apple-darwin",
  "linux-x64": "x86_64-unknown-linux-gnu",
  "linux-arm64": "aarch64-unknown-linux-gnu",
  "win32-x64": "x86_64-pc-windows-msvc",
};

function fail(message) {
  console.error(`surrealguard: ${message}`);
  console.error(`See ${RELEASES_PAGE} for available prebuilt binaries.`);
  process.exit(1);
}

function resolveTarget() {
  const key = `${process.platform}-${process.arch}`;
  const target = TARGETS[key];
  if (!target) {
    fail(
      `unsupported platform/arch: ${key}. Supported: ${Object.keys(TARGETS).join(", ")}.`
    );
  }
  return target;
}

function assetName(target, ext) {
  return `surrealguard-${VERSION}-${target}.${ext}`;
}

function downloadUrl(target, ext) {
  return `https://github.com/${REPO}/releases/download/v${VERSION}/${assetName(target, ext)}`;
}

// Follow redirects (GitHub release assets 302 to a CDN) and stream to disk.
function download(url, dest, redirectsLeft = 10) {
  return new Promise((resolve, reject) => {
    if (redirectsLeft < 0) {
      reject(new Error("too many redirects"));
      return;
    }
    const request = https.get(
      url,
      { headers: { "User-Agent": "surrealguard-npm-launcher" } },
      (res) => {
        const { statusCode, headers } = res;
        if (statusCode >= 300 && statusCode < 400 && headers.location) {
          res.resume();
          const next = new URL(headers.location, url).toString();
          download(next, dest, redirectsLeft - 1).then(resolve, reject);
          return;
        }
        if (statusCode !== 200) {
          res.resume();
          reject(new Error(`HTTP ${statusCode} for ${url}`));
          return;
        }
        const file = fs.createWriteStream(dest);
        res.pipe(file);
        file.on("finish", () => file.close(() => resolve()));
        file.on("error", (err) => {
          fs.rm(dest, { force: true }, () => reject(err));
        });
      }
    );
    request.on("error", reject);
  });
}

function extract(archive, ext, destDir) {
  if (ext === "zip") {
    // Windows: unpack with PowerShell (no runtime deps).
    const result = spawnSync(
      "powershell.exe",
      [
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        `Expand-Archive -Path '${archive}' -DestinationPath '${destDir}' -Force`,
      ],
      { stdio: "inherit" }
    );
    if (result.status !== 0) {
      throw new Error("failed to extract zip archive with Expand-Archive");
    }
  } else {
    // Unix: system tar is present on macOS and Linux (and modern Windows).
    const result = spawnSync("tar", ["-xzf", archive, "-C", destDir], {
      stdio: "inherit",
    });
    if (result.status !== 0) {
      throw new Error("failed to extract tar.gz archive with tar");
    }
  }
}

async function ensureBinary(target) {
  const isWindows = process.platform === "win32";
  const ext = isWindows ? "zip" : "tar.gz";
  const binName = isWindows ? "surrealguard.exe" : "surrealguard";

  const cacheDir = path.join(__dirname, "..", "binaries", target);
  const binPath = path.join(cacheDir, binName);
  if (fs.existsSync(binPath)) {
    return binPath;
  }

  fs.mkdirSync(cacheDir, { recursive: true });
  const url = downloadUrl(target, ext);
  const archivePath = path.join(
    os.tmpdir(),
    `${assetName(target, ext)}.${process.pid}`
  );

  process.stderr.write(`surrealguard: downloading ${url}\n`);
  try {
    await download(url, archivePath);
    extract(archivePath, ext, cacheDir);
  } catch (err) {
    fail(
      `failed to download prebuilt binary for ${target} (v${VERSION}).\n` +
        `  ${err.message}`
    );
  } finally {
    fs.rm(archivePath, { force: true }, () => {});
  }

  if (!fs.existsSync(binPath)) {
    fail(
      `downloaded archive did not contain the expected binary '${binName}' for ${target}.`
    );
  }
  if (!isWindows) {
    fs.chmodSync(binPath, 0o755);
  }
  return binPath;
}

async function main() {
  const target = resolveTarget();
  const binPath = await ensureBinary(target);
  try {
    execFileSync(binPath, process.argv.slice(2), { stdio: "inherit" });
  } catch (err) {
    if (typeof err.status === "number") {
      process.exit(err.status);
    }
    fail(`failed to execute binary: ${err.message}`);
  }
}

main();
