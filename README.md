# TwinCursor

[![CI](https://github.com/abhinavbolla/twincursor/actions/workflows/ci.yml/badge.svg)](https://github.com/abhinavbolla/twincursor/actions/workflows/ci.yml)
[![License: AGPL-3.0](https://img.shields.io/badge/license-AGPL--3.0-blue.svg)](LICENSE)

**Open-source remote pair programming.** Share your screen with sharp text and very little lag,
talk, and let your pair take the keyboard and mouse. Pairing over the internet should feel like
sharing a desk.

> **TwinCursor is in early development and there's no app to install yet.** Screen sharing works
> from the command line, and remote control is being built now. See [what works](#what-works-today).

## Why TwinCursor

The best pairing tools are closed and paid per seat, and general video calls blur code and lag
behind your hands. TwinCursor is built around three ideas:

- **Latency is the product.** All media runs in native Rust rather than a webview, and every change
  to that path has to report its effect on latency.
- **You stay in control of your machine.** A guest can only control your computer after you grant
  it. You can always see who is controlling it, and revoke it instantly with a global hotkey.
- **It's yours to run.** It's AGPL-3.0 and self-hostable, and Linux will be a first-class platform.

## What works today

| | Status |
|---|---|
| Screen sharing | ✅ Works from the command line: macOS sharer; macOS or Windows viewer |
| Call health stats and on-screen HUD | ✅ Works |
| Auto-throttle on poor networks | ✅ Works |
| Remote control | 🚧 In progress |
| Voice | Planned |
| Desktop app | Planned |
| Self-hosting with one command | Planned |
| Linux | Planned |

The [roadmap](docs/ROADMAP.md) has the full plan.

## Measured performance

| Measure | Result |
|---|---|
| 1080p between two machines on home Wi-Fi | 60 fps, round trip 29 ms typical, 105 ms at p95 |
| Text quality, H.264 | SSIM 0.9996 (1.0 is identical) at about 0.8 Mbps, even while scrolling fast |
| Sharer CPU, H.264 at 1080p | 14% of one core |
| Delay the input path adds | Under 1 ms on top of the network |

These come from one Apple M4 MacBook and one Windows laptop, and the
[performance page](docs/performance.md) has the full results and how to reproduce them. They don't
yet include the app's own redraw time, so they aren't full glass-to-glass latency.

## Try it

Today's build is `tc-spike`, a command-line tool for sharing a screen and measuring the result.

You'll need an Apple Silicon Mac with [Xcode 26](https://developer.apple.com/xcode/) and
[Rust](https://rustup.rs). The first build downloads a prebuilt WebRTC library of about 277 MB.

```bash
brew install livekit
git clone https://github.com/abhinavbolla/twincursor
cd twincursor
cargo build --release -p twincursor-media-spike
```

Then run each of these in its own terminal, from the `twincursor` folder:

```bash
livekit-server --dev
```

```bash
target/release/tc-spike share
```

```bash
target/release/tc-spike view
```

macOS will ask you to allow Screen Recording for your terminal. Press `S` in the viewer to show or
hide the stats HUD. Run `tc-spike --help` to see the options, such as `--resolution 1440`.

### Between two machines

Start the server so that other machines can reach it, then share as above:

```bash
livekit-server --dev --bind 0.0.0.0
```

On the other machine, build `tc-spike` the same way and point it at the sharer's IP address:

```bash
target/release/tc-spike view --url ws://192.168.1.20:7880
```

`--dev` uses well-known keys and an unencrypted connection to the server, so only do this on a
network you trust.

To build the viewer on Windows, use the Rust MSVC toolchain and clone to a short path such as
`C:\tc`, because deep folders hit git's filename limit.

## How it works

The sharer captures the screen with ScreenCaptureKit and encodes it, using hardware H.264 by
default. Video goes through a [LiveKit](https://livekit.io) server to a native viewer, and
remote-control input comes back over data channels. Read the
[architecture](docs/ARCHITECTURE.md) for the details, and the
[decision records](docs/adr) for why it's built this way.

## Contributing

Issues, measurements and pull requests are all welcome. Start with
[CONTRIBUTING.md](CONTRIBUTING.md), and look for a
[good first issue](https://github.com/abhinavbolla/twincursor/labels/good%20first%20issue). To ask a
question or share an idea, use [Discussions](https://github.com/abhinavbolla/twincursor/discussions).
Everyone taking part follows our [code of conduct](CODE_OF_CONDUCT.md).

Report security problems privately: see [SECURITY.md](SECURITY.md).

## License

TwinCursor is licensed under [AGPL-3.0](LICENSE). Contributions are accepted under a
[Contributor License Agreement](CLA.md).
