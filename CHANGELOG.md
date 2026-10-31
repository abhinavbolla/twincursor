# Changelog

Notable changes to TwinCursor. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions will follow
[Semantic Versioning](https://semver.org). There are no releases yet.

## [Unreleased]

### Added

- Screen sharing from macOS with ScreenCaptureKit, through a LiveKit server, to a native viewer
  on macOS or Windows.
- H.264 as the default codec, with VP8 as the fallback, and resolutions up to 1440p and native.
- Auto-throttle that gives up frame rate before resolution, and caps the bitrate when the
  viewer's downlink can't keep up.
- Call health stats on both ends, and an on-screen HUD in the viewer.
- Measurement tools: latency markers, text quality against a synthetic code editor, input path
  pings, and a Docker network lab with pass/fail targets.

### Fixed

- Frame pacing no longer drops frames at the full frame rate.
- The sharer keeps only the newest frame when the encoder falls behind, instead of queueing them.
