# image-reader

Finds a product from a reference photo in a video. It samples the video's frames
and asks TypeSafe's Jev model about each one: is the product in this frame, and
where is it?

```bash
./image-reader reference/starfox.png https://www.youtube.com/shorts/K1WHleqWkJw   # positive case
./image-reader reference/starfox.png https://www.youtube.com/shorts/G79nGWHKogU   # negative case
```

Exit code `0` means it found the product, `1` means it didn't, and `2` means an error.
Matching frames are saved twice to `output/<video>/`: once with the product outlined,
and once cropped to it.

## Setup

- Rust (`rustup`), plus `ffmpeg` and `yt-dlp` on your `PATH`.
- A TypeSafe API key in `.env` (git-ignored): `TYPESAFE_API_KEY=...`

`./image-reader` is a wrapper around `cargo run --release`, so it rebuilds when the code changes.
`./image-reader --help` lists the options (`--fps`, `--threshold`, `--describe`, `--json`, ...).
`cargo test` runs the unit tests.

## How it works

1. **`src/video.rs`**: downloads a URL with `yt-dlp`, reads the size and duration with
   `ffprobe`, and samples JPEG frames (default 2 per second) with `ffmpeg`. The frames
   are sized to fit the image API's limits (each side 256–1568 px).
2. **`src/images.rs`**: turns the reference image upright and sizes it within the same
   limits. For matching frames, it draws and crops the box.
3. **`src/jev.rs`**: one multipart request per frame. The reference and the frame go into
   `state` as images, with two questions:
   - `product_visible`, a **Noul**: the probability that the frame shows the same product.
     Packaging, posters, and footage on a screen don't count.
   - `product_box`, a **Box**: where the product is. A Box always returns a box, so the
     code ignores it unless the Noul passes `--threshold`.

   Every request sends the `TypeSafe-Danger-Zone-Images: true` header. Rate-limit and
   server errors are retried with backoff.
4. **`src/report.rs`**: prints the per-frame table and the verdict, or JSON.
5. **`src/main.rs`**: the command-line arguments, and the code that ties the steps together.

Images and Box are experimental TypeSafe features. Box needs `jev-1.14.0` (the default `--model`).
