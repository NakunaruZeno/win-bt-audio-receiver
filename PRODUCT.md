# Product

<!-- impeccable:product-schema 1 -->

## Platform

adaptive

## Users

Windows PC users who want ultra-low-latency, lossless wireless audio routed from their Android or iPhone/iPad directly to their PC speakers or headphones over 5 GHz Wi-Fi (or Bluetooth A2DP fallback), without heavy bloated desktop suites or paid third-party streaming apps.

## Product Purpose

AetherSora turns a Windows PC into a lightweight universal wireless audio receiver (`AetherSora.exe`) paired with minimal zero-bloat mobile companion senders:

- **Windows Receiver (`AetherSora.exe`):** Ultra-lightweight pure-Rust Win32 system tray utility (~438 KB, ~2–6 MB RAM, zero external Rust crates beyond `windows-rs`) featuring Auto/Wi-Fi/Bluetooth transport arbitration and a Celestial Sky Flyout.
- **Android Companion (`AetherSora-Companion.apk`):** Pure Android SDK client (~55 KB, zero Gradle overhead) using `AudioPlaybackCaptureConfiguration` (Android 10+) to stream internal media/game audio over UDP without Developer Mode or ADB.
- **iOS / iPadOS Companion (`AetherSoraCompanion.ipa`):** Native SwiftUI + ReplayKit Broadcast Upload Extension client (`RPBroadcastSampleHandler`) capturing `.audioApp` system audio over UDP to Windows PC, built automatically into an unsigned `.ipa` on GitHub Actions (`macos-15`) for AltStore, SideStore, Sideloadly, or TrollStore.

## Positioning

Unlike heavy Electron/Qt desktop audio receivers or ad-supported mobile streaming apps, AetherSora pairs a sub-500 KB native Windows tray binary with tiny, ad-free mobile senders sharing one wire-compatible 48 kHz 16-bit Stereo PCM UDP protocol (`0xA55A` magic, 5 ms packets, LAN UDP `19481` auto-discovery).

## Operating Context

1. Phone and Windows PC join the same local 5 GHz Wi-Fi network (or Wi-Fi hotspot).
2. AetherSora runs on Windows in `Auto` or `Wi-Fi 5GHz (Companion App)` mode, passively listening for `AETHERSORA_DISCOVER_V1` on UDP `19481` and audio on UDP `19480`.
3. On Android or iOS, the user opens the AetherSora companion app, taps a discovered PC card (or enters IP manually), and starts streaming in one action.
4. Mobile streaming stays active in the background while using YouTube, games, browsers, or podcasts.

## Capabilities and Constraints

- **Audio Protocol:** 48,000 Hz, 16-bit little-endian stereo PCM, 240 frames (960 bytes PCM + 12-byte header = 972 bytes/packet, 5 ms chunks), `HELLO`/`ACK`/`KEEPALIVE`/`BYE` control flags, DSCP `0xB8`.
- **Android Auto-Mute:** Automatically lowers `STREAM_MUSIC` to `0` on start (when enabled) and restores prior volume cleanly on disconnect.
- **iOS ReplayKit Behavior:** Captures `.audioApp` system/media audio pre-speaker; Apple does not allow apps to change hardware media volume programmatically, so lowering the iPhone hardware volume to `0` eliminates phone echo while streaming 100% audio to PC. FairPlay DRM apps (e.g., Apple Music, Netflix) are silenced by iOS during ReplayKit broadcasts by OS policy.
- **Free Apple ID Sideloading:** Sideloading tools rewrite Team IDs and often strip `App Groups` entitlements; the iOS companion includes a dual-path loopback UDP bridge (`127.0.0.1:19489`) and extension-side LAN auto-discovery fallback so streaming and live RMS telemetry still work without paid developer entitlements.

## Brand Commitments

- **Theme:** Deep night sky (`#091026`–`#0D1633`), Sora Sky Azure (`#38BDF8`), Starlight Gold (`#FACC15`), anchored on the Chrono-Star Portal mark (`assets/logo.svg`).
- **Voice:** Calm, concise, technical-clear UI copy with actionable error recovery and zero promotional filler.

## Evidence on Hand

- `assets/logo.svg`, `assets/logo-symbol.svg` (brand vector identity)
- `src/wifi/protocol.rs`, `src/wifi/discovery.rs`, `src/wifi/mod.rs` (verified UDP protocol & Windows WASAPI receiver)
- `src/tray/flyout.rs` (Celestial Sky desktop flyout reference)
- `android/companion/` (existing pure-SDK APK builder & capture service)
