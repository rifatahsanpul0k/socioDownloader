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
cargo run -p downloader-cli -- bootstrap
cargo run -p downloader-cli -- resolve <URL> [--playlist]
cargo run -p downloader-cli -- download <URL> [--playlist] [--format ID] [--output DIR]
cargo run -p downloader-cli -- resolve <URL> --cookies-from-browser safari
cargo run -p downloader-cli -- download <URL> --cookies /path/to/cookies.txt
cargo run -p downloader-cli -- update
```

Batch input may contain multiple URLs separated by whitespace, commas, or
newlines. `--playlist` expands YouTube playlists and watch links containing a
playlist; Instagram `/p/` carousel posts and explicit YouTube playlist URLs are
expanded automatically.

## GUI

```sh
cargo run -p rust-dl-gui
```

Built with `eframe`/`egui` strictly following Apple Human Interface Guidelines (HIG) and Jakob Nielsen's Usability Heuristics:
- **macOS Window Shell**: Transparent titlebar with Cocoa traffic light insets and San Francisco (SF Pro) typography.
- **Pre-Queue Staging Card**: Interactive card showing video thumbnail, duration, uploader, stream badges (`Highest Quality 1080p Video + High-Res Audio`, `🎬 H.264 + 🎵 AAC`), and automatic pre-download disk space verification.
- **Three-Stage Visual Progress**: Distinct dynamic phases (Probing, Multi-chunk Downloading, FFmpeg Muxing, and Completed).
- **Actionable Error Recovery**: Plain-language error masking with one-click cookie authentication shortcuts for protected media.
- **Non-intrusive Toasts & History**: Automatic clipboard link detection notification and 5-second undo toast for completed downloads.

## macOS `.app` bundle

Build the release GUI and create `rust-dl.app` with:

```sh
cargo build --workspace --release
cargo run -p downloader-cli -- bundle-app --output-dir dist
```

The command creates `dist/rust-dl.app/Contents` with `MacOS/rust-dl`,
`Resources`, and a macOS `Info.plist` targeting macOS 12.0 or newer. A DMG can
then be created with the macOS Disk Utility or:

```sh
hdiutil create -volname rust-dl -srcfolder dist/rust-dl.app -ov -format UDZO dist/rust-dl.dmg
```

## License

MIT
