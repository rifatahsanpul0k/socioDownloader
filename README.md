# rust-dl

`rust-dl` is an open-source macOS media downloader written in Rust. It uses a
small native resolver/downloader for direct media and yt-dlp as an authenticated
fallback, with FFmpeg handling DASH and HLS muxing.

## Architecture

- `crates/core`: URL sanitization, batch and playlist resolution, browser-cookie
  forwarding, sidecar management, concurrent downloads, atomic config/history,
  and macOS app bundling.
- `crates/cli`: bootstrap, resolve, download, extractor update, and bundle
  commands.
- `crates/gui`: an `eframe`/`egui` desktop queue with clipboard and drag/drop
  input, settings, history, and background extractor updates.

## Build

```sh
cargo check --workspace
cargo build --workspace --release
```

The GUI binary is `target/release/rust-dl-gui`. On macOS, the first run can
download managed yt-dlp and FFmpeg sidecars into
`~/Library/Application Support/rust-dl/bin`.

## CLI usage

```sh
cargo run -p rust-dl-cli -- bootstrap
cargo run -p rust-dl-cli -- resolve <URL> [--playlist]
cargo run -p rust-dl-cli -- download <URL> [--playlist] [--format ID] [--output DIR]
cargo run -p rust-dl-cli -- resolve <URL> --cookies-from-browser safari
cargo run -p rust-dl-cli -- download <URL> --cookies /path/to/cookies.txt
cargo run -p rust-dl-cli -- update
```

Batch input may contain multiple URLs separated by whitespace, commas, or
newlines. `--playlist` expands YouTube playlists and watch links containing a
playlist; Instagram `/p/` carousel posts and explicit YouTube playlist URLs are
expanded automatically.

## GUI

```sh
cargo run -p rust-dl-gui
```

Paste with **Paste & Download** or Cmd+V, enable clipboard capture, or drag a
URL/text file into the window. The toolbar controls cookie sources, quality,
concurrency, playlist/carousel expansion, and yt-dlp updates.

## macOS `.app` bundle

Build the release GUI and create `rust-dl.app` with:

```sh
cargo build --workspace --release
cargo run -p rust-dl-cli -- bundle-app --output-dir dist
```

The command creates `dist/rust-dl.app/Contents` with `MacOS/rust-dl`,
`Resources`, and a macOS `Info.plist` targeting macOS 12.0 or newer. A DMG can
then be created with the macOS Disk Utility or:

```sh
hdiutil create -volname rust-dl -srcfolder dist/rust-dl.app -ov -format UDZO dist/rust-dl.dmg
```

## License

MIT
