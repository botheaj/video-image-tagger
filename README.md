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

1. Install Rust (via [rustup](https://rustup.rs)), and put `ffmpeg` and `yt-dlp` on your `PATH`.
2. **Create a `.env` file. The tool won't run without it.** Get an API key from the
   [TypeSafe console](https://console.typesafe.ai), then, from the repo folder:

   ```bash
   echo 'TYPESAFE_API_KEY=your-key-here' > .env
   chmod 600 .env
   ```

   `.env` is git-ignored, so it is never committed. The tool reads it at startup, so
   run `./image-reader` from the repo folder. If `TYPESAFE_API_KEY` is already exported
   in your shell, that value is used instead. Without a key, the tool stops with
   `TYPESAFE_API_KEY is not set` (exit code 2).

`./image-reader` is a wrapper around `cargo run --release`, so it rebuilds when the code changes.
`./image-reader --help` lists the options (`--fps`, `--threshold`, `--describe`, `--json`, ...).
`cargo test` runs the unit tests.

## Using it from a coding agent

Copy this prompt into Claude Code or another coding agent, and fill in the two placeholders:

```text
Use the image-reader CLI (https://github.com/botheaj/image-reader) to check whether
the product in <PATH TO PRODUCT IMAGE> appears in <VIDEO FILE PATH OR URL>.

1. If it isn't already present, clone the repo and cd into it. Make sure `cargo`, `ffmpeg`
   and `yt-dlp` are installed; if any is missing, tell me what to install instead of
   installing system packages yourself.
2. The tool needs a `.env` file in the repo folder containing TYPESAFE_API_KEY=<key>.
   If it doesn't exist, ask me for the key or ask me to create the file myself. Never
   print the key, echo it into logs, or commit `.env`.
3. Run: ./image-reader --json <product image> <video>
   Optional flags: --fps (frames per second sampled, default 2), --threshold (default 0.5),
   --describe "<short product description>" (helps with ambiguous products).
   Each sampled frame costs one API request, so ask me before raising --fps or --max-frames.
4. Interpret the result. Exit code 0 means the product was found, 1 means it was not found,
   and 2 means an error (report the error message). From the JSON, tell me the verdict, when the product
   first appears, and the strongest frame. For matches, look at the outlined images under
   output/<video name>/ and confirm the box is really on the product.
5. If frames near the threshold look wrong, suggest a --threshold or --describe change and
   offer to rerun. Don't rerun on your own.
```

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
