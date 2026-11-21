<p align="center">
  <img src="assets/logo.svg" width="128" height="128" alt="AetherSora Logo">
</p>

# AetherSora: Windows Hybrid Wi-Fi 5GHz LAN & Bluetooth Audio Receiver

AetherSora is an ultra-lightweight Windows console application written in Rust that turns your Windows PC into a high-quality wireless speaker for your phone. It combines a **48 kHz 16-bit Stereo Lossless Wi-Fi LAN mode** (via Android Wireless Debugging, with zero APK installed) and an automatic, strictly isolated **Bluetooth A2DP Sink fallback** when Wi-Fi is unavailable (such as when using mobile data).

---

## Mode Comparison

AetherSora enforces strict compile-time mutual exclusion (`ActiveTransport`): **only one transport backend exists in memory at a time**. When Wi-Fi mode runs, zero Bluetooth WinRT handles or watchers are created; when Bluetooth fallback runs, zero UDP sockets, WASAPI threads, or ADB processes exist.

| Feature | Wi-Fi 5 GHz LAN Mode (Primary) | Bluetooth A2DP Sink Mode (Fallback) |
| :--- | :--- | :--- |
| **Audio Quality** | **48 kHz 16-bit Stereo Lossless PCM** (1.536 Mbps) | Standard Windows SBC Stereo (44.1 kHz, ~328 kbps) |
| **Latency** | ~15 - 30 ms (suitable for video and gaming) | ~150 - 250 ms |
| **2.4 GHz BLE Mouse/Keyboard Interference** | None (runs on 5 GHz Wi-Fi LAN) | Possible on shared combo Wi-Fi/BT adapters |
| **PC Resource Footprint** | ~1.8 - 4 MB RAM, < 0.5% CPU | ~4.2 - 4.6 MB RAM, 0% CPU idle |
| **Phone Requirement** | Android 11+ with Wireless Debugging (no APK) | Any Bluetooth-paired Android or iOS device |

---

## System Requirements

- **Operating System:** Windows 10 Version 2004 (Build 19041) or newer, or Windows 11.
- **For Wi-Fi 5 GHz LAN Mode (Optional, Android 11+):**
  - PC and phone connected to the same local network (5 GHz Wi-Fi strongly recommended).
  - Android Platform Tools (`adb.exe`) available in `PATH` or `%LOCALAPPDATA%\Android\Sdk\platform-tools\adb.exe`.
- **For Bluetooth Fallback Mode (Android & iOS):**
  - A Windows Bluetooth adapter supporting A2DP Sink.
- **For Building from Source:**
  - Rust toolchain (`rustc` / `cargo` via `rustup`) with the `x86_64-pc-windows-msvc` target.

---

## Installation

### Option A: Install Directly via Cargo (Fastest)

If you already have Rust installed:

```powershell
cargo install --git https://github.com/NakunaruZeno/win-bt-audio-receiver.git
```

Once finished, you can run `aethersora` from any terminal.

### Option B: Build from Source

1. Install the Rust toolchain (skip if already installed, then reopen PowerShell):
   ```powershell
   winget install Rustlang.Rustup
   ```

2. Clone the repository and build the optimized release executable:
   ```powershell
   git clone https://github.com/NakunaruZeno/win-bt-audio-receiver.git
   cd win-bt-audio-receiver
   cargo build --release
   ```

3. Your standalone executable (~368 KB, with the 5.3 KB Android agent and Windows icon embedded inside) is ready at:
   ```text
   target\release\aethersora.exe
   ```

---

## Step-by-Step Setup Guide

### Part 1: Bluetooth Setup (Works on Android & iOS, Required for Fallback)

1. On Windows, open **Settings** (`Win + I`) -> **Bluetooth & devices** -> **Add device** -> **Bluetooth**, and pair your phone.
2. Run AetherSora once in Bluetooth mode to scan and cache your phone's Device ID:
   ```powershell
   .\target\release\aethersora.exe --mode bt --scan
   ```
3. Select your phone from the list. AetherSora saves it to `device_id.cfg` next to the executable so future Bluetooth connections open automatically.

### Part 2: Wi-Fi 5 GHz LAN Lossless Setup (Optional, Android 11+ Only)

No APK needs to be installed on your phone. AetherSora pushes a temporary 5.3 KB capture agent into RAM over ADB and automatically cleans it up when you exit.

1. **Install ADB on Windows** (skip if you already have Android Studio or Platform Tools):
   ```powershell
   winget install Google.PlatformTools
   ```
   *(Reopen PowerShell after installing so `adb` is recognized. AetherSora also auto-detects `%LOCALAPPDATA%\Android\Sdk\platform-tools\adb.exe`.)*

2. **Enable Wireless Debugging on Android:**
   - Open **Settings -> About phone** and tap **Build number** (or **OS version** on Xiaomi HyperOS/MIUI) 7 times to unlock **Developer options**.
   - Open **Settings -> Developer options -> Wireless debugging** and toggle it **On**.
   - When prompted for your home Wi-Fi, check **"Always allow on this network"**. *(For security, Android automatically turns off Wireless Debugging whenever you leave your home Wi-Fi or switch to mobile data.)*

3. **Pair your PC and Android phone once:**
   - Inside the **Wireless debugging** screen, tap **Pair device with pairing code**.
   - A popup will appear displaying a **6-digit pairing code** and a **Pairing IP address & Port** (note: this pairing port is different from the port shown on the outer Wireless Debugging screen).
   - **Keep that popup open on your phone screen**, then run on your PC:
     ```powershell
     .\target\release\aethersora.exe --pair <IP:Pairing_Port> <6_Digit_Code>
     ```
     Example:
     ```powershell
     .\target\release\aethersora.exe --pair 192.168.1.50:41235 123456
     ```

4. **Start streaming in Auto mode:**
   ```powershell
   .\target\release\aethersora.exe
   ```
   - On first launch, if **Windows Defender Firewall** asks for network permission, click **Allow access** so AetherSora can receive audio UDP packets on port `19480`.
   - Your phone will automatically be discovered over LAN mDNS (`_adb-tls-connect._tcp`) and begin streaming uncompressed 48 kHz Stereo PCM to your PC speakers.

---

## Daily Usage & CLI Reference

```powershell
# Default Auto Mode: tries Wi-Fi 5 GHz LAN first; falls back to Bluetooth if Wi-Fi is unavailable
.\aethersora.exe

# Force Wi-Fi 5 GHz LAN mode only (never initializes Bluetooth)
.\aethersora.exe --mode wifi

# Force Bluetooth A2DP Sink mode only (skips Wi-Fi/ADB probe completely)
.\aethersora.exe --mode bt

# Manually connect and cache a specific Android Wireless Debugging IP:Port
.\aethersora.exe --adb-host 192.168.1.50:37329

# Set the Wi-Fi WASAPI jitter buffer target in milliseconds (range: 10..80, default: 20)
.\aethersora.exe --buffer-ms 20

# Re-scan paired Bluetooth devices and overwrite cached Bluetooth device in device_id.cfg
.\aethersora.exe --scan

# List all paired Bluetooth audio source devices and exit
.\aethersora.exe --list

# Connect directly to a paired Bluetooth device matching a name or ID substring
.\aethersora.exe --device "Phone"

# Disable on-demand Bluetooth DeviceWatcher auto-reconnect for minimal footprint
.\aethersora.exe --no-reconnect

# Display CLI help
.\aethersora.exe --help
```

Press `Ctrl+C` (or close the console window) to cleanly close the active audio stream and immediately restore sound to your phone's internal speaker.

### How Automatic Fallback Works (`--mode auto`)

1. At startup, AetherSora quickly probes for your paired Android phone on the local Wi-Fi network via ADB mDNS / cached host.
2. If found, it launches `WifiSession` (48 kHz Lossless PCM over UDP -> WASAPI Event-Driven `"Pro Audio"` renderer). Bluetooth is never touched.
3. If the phone is not on Wi-Fi (e.g., on 4G/5G mobile data) or drops off Wi-Fi mid-session (> 2.5 s with no audio or keepalive packets), AetherSora completely tears down `WifiSession` first (releasing WASAPI, UDP, and ADB), waits 200 ms, and seamlessly starts `BtSession` (Bluetooth A2DP Sink).

---

## Troubleshooting

### `adb pair` fails with `error: protocol fault (couldn't read status message)`
- Verify the IP address has no typos (e.g., `192.168.x.x:port`).
- Make sure you are using the **port inside the "Pair device with pairing code" popup**, not the outer port on the main Wireless Debugging page.
- Keep the pairing popup open and the phone screen awake until the command succeeds.

### Connected in Wi-Fi mode, but no sound plays on the PC
1. **Disconnect Bluetooth audio on your phone:** If your phone is simultaneously connected to a Bluetooth audio device (including the PC's own Bluetooth audio profile), Android routes media to Bluetooth instead of `REMOTE_SUBMIX`. Disconnect Bluetooth media on the phone or switch media output away from Bluetooth.
2. **Allow UDP port `19480` in Windows Defender Firewall:** Especially if your Wi-Fi network profile in Windows is set to **Public**, ensure `aethersora.exe` is allowed for Inbound UDP traffic.
3. **Check phone media volume:** Android's `REMOTE_SUBMIX` output follows the phone's media volume level; turn up media volume on the phone.
4. **Xiaomi HyperOS / MIUI devices:** If the Android agent reports `ERR:AUDIO_INIT_FAILED`, enable **USB debugging (Security settings)** inside **Developer options** and ensure the screen is unlocked when starting the stream.

### Audio stutters in Bluetooth fallback mode
- Open **Device Manager -> Bluetooth -> [Your Bluetooth Adapter] -> Properties -> Power Management** and uncheck **"Allow the computer to turn off this device to save power"**.
- Disable **USB selective suspend** in Windows **Control Panel -> Power Options -> Advanced power settings -> USB settings**.
- Connect both PC and phone to **5 GHz Wi-Fi** to avoid 2.4 GHz radio congestion. If you use a combo Wi-Fi/Bluetooth card alongside a BLE mouse/keyboard, prefer Wi-Fi LAN mode (`--mode wifi`) to eliminate Bluetooth radio contention completely.

### Two-way voice calls / Discord voice on phone
- Bluetooth A2DP Sink is a unidirectional media profile (no HFP microphone return), so Android voice apps in communication mode may route audio to the phone earpiece/speaker instead of the PC.

---

## Configuration File (`device_id.cfg`)

AetherSora stores its local cache in `device_id.cfg` next to the executable (excluded from git via `.gitignore`):

1. **Line 1:** Cached Bluetooth `DeviceId`
2. **Line 2:** Cached Bluetooth display name
3. **Line 3 (optional):** Last known Android Wireless Debugging IP / `IP:Port`

Delete `device_id.cfg` or run with `--scan` / `--adb-host` at any time to reset it.

---

## Architecture Summary

- **`src/main.rs`:** CLI parser and compile-time isolated `ActiveTransport` orchestrator (`WifiLan` vs `Bluetooth`).
- **`src/wifi/`:** 12-byte UDP packet framing (`protocol.rs`, 5 ms / 972-byte datagrams with DSCP EF QoS and 400 ms silence keepalives), 100% safe lock-free SPSC ring buffer with Packet Loss Concealment (`ringbuf.rs`), WASAPI Event-Driven `"Pro Audio"` renderer with `IAudioClockAdjustment` drift compensation (`wasapi.rs`), and ADB Wireless discovery/lifeline manager (`adb.rs`).
- **`android-agent/`:** Minimal Java capture agent compiled to a 5.3 KB DEX (`aether-agent.dex`) embedded directly into `aethersora.exe`.
- **`src/bluetooth/`:** Isolated WinRT `AudioPlaybackConnection` receiver with on-demand `DeviceWatcher`.
- **`src/signal.rs`:** Console signal handling (`SetConsoleCtrlHandler`), Windows 11 EcoQoS power-throttling opt-out, and active-stream keep-awake (`SetThreadExecutionState`).

---

## License

This project is licensed under the [MIT License](LICENSE).
