<p align="center">
  <img src="assets/logo.svg" width="128" height="128" alt="AetherSora Logo">
</p>

# AetherSora: Windows System Tray Universal Audio Receiver

AetherSora is an ultra-lightweight (~422 KB) Windows System Tray audio receiver written in Rust with **zero third-party Rust crates** beyond Microsoft's official `windows` crate. It launches silently into the Windows System Tray (zero terminal window) with native Windows Toast notifications and turns your PC into a low-latency wireless speaker over **Wi-Fi 5 GHz (48 kHz Lossless PCM)** or **Bluetooth A2DP Sink**.

---

## Download & Installation

### Requirements
- **PC:** Windows 10 (Version 2004 / Build 19041+) or Windows 11 (`x86_64-pc-windows-msvc` Rust toolchain to build from source).
- **Phone:**
  - **Bluetooth mode:** Any Android or iOS phone (zero phone app required).
  - **Wi-Fi Companion App mode:** Android 10+ on the same Wi-Fi LAN (no Developer Mode, no ADB).
  - **Wi-Fi ADB Wireless mode:** Android 11+ on the same Wi-Fi LAN + `adb.exe` on PC (`winget install Google.PlatformTools`).

### Install PC Receiver (`AetherSora.exe`)

**Option A — Install directly with Cargo:**
```powershell
cargo install --git https://github.com/NakunaruZeno/win-bt-audio-receiver.git
```

**Option B — Clone and build from source:**
```powershell
git clone https://github.com/NakunaruZeno/win-bt-audio-receiver.git
cd win-bt-audio-receiver
cargo build --release
```

The standalone executable (~422 KB, with pure Win32 System Tray, embedded 5.3 KB ADB agent, and Chrono-Star icon) is produced at:
```text
target\release\AetherSora.exe
```

### Android Companion App (Optional — For Wi-Fi Lossless Without Developer Mode)

A pre-built, signed **24.5 KB** APK is included in this repository:
```text
android\companion\AetherSora-Companion.apk
```
Copy it to your Android 10+ phone and install directly. *(To rebuild the APK locally using your installed Android SDK + JDK, run `powershell -ExecutionPolicy Bypass -File .\android\companion\build.ps1`.)*

---

## Quick Setup Guide

Double-click `AetherSora.exe` to start immediately in the **Windows System Tray** (default mode is **Auto** — no terminal window opens, and Windows Toast notifications report connection status).

Pick the connection path that fits your device:

### Option 1: Wi-Fi 5 GHz Companion App (Android 10+ — Recommended for Most Users)
1. Start `AetherSora.exe` on your PC in **Auto** or **Wi-Fi 5GHz (Companion App)** mode.
   - If Windows Defender Firewall prompts on first launch, click **Allow access** (UDP `19480` audio + UDP `19481` LAN discovery).
2. Install and open `android\companion\AetherSora-Companion.apk` on your phone. It automatically discovers your PC on Wi-Fi LAN.
3. Tap **Connect & Stream Lossless** and accept Android's capture prompt.
   - **Auto-mute phone speaker** is enabled by default to set phone media volume to `0` (eliminating speaker echo) and automatically restores your previous volume when you disconnect. *(Uncheck it if your Android ROM silences internal capture at volume 0.)*

### Option 2: Bluetooth A2DP Sink (Android & iOS — Zero Phone App)
1. Pair your phone with Windows via **Settings (`Win + I`) -> Bluetooth & devices -> Add device -> Bluetooth**.
2. Right-click the AetherSora icon in the System Tray -> **Bluetooth Device -> Scan / Refresh Paired Devices**, then click your phone.

### Option 3: Wi-Fi 5 GHz ADB Wireless (Android 11+ — Zero APK, All Apps, Auto-Mute)
1. Install ADB on Windows if not already installed:
   ```powershell
   winget install Google.PlatformTools
   ```
   *(AetherSora also auto-detects `%LOCALAPPDATA%\Android\Sdk\platform-tools\adb.exe`.)*
2. On Android, enable **Settings -> Developer options -> Wireless debugging** and check **"Always allow on this network"** for your home Wi-Fi.
3. Tap **Pair device with pairing code**, keep the popup open, and pair once using the IP and **pairing port inside the popup**:
   ```powershell
   .\target\release\AetherSora.exe --pair 192.168.1.50:41235 123456
   ```
4. Double-click `AetherSora.exe` — Auto mode connects over 5 GHz Wi-Fi automatically.

---

## System Tray Controls & Transport Modes

**Click or right-click** the Chrono-Star icon in the System Tray to:
- **Hot-switch modes:** `Auto`, `Wi-Fi 5GHz (Companion App)`, `Wi-Fi 5GHz (ADB Wireless)`, or `Bluetooth A2DP Sink`.
- **Bluetooth Device:** Select any paired phone directly or run **Scan / Refresh Paired Devices**.
- **Start with Windows:** Toggle auto-launch at sign-in (`HKCU\...\Run\AetherSora`).
- **Show Debug Log:** Open a native Win32 read-only live log viewer (closing the window only hides it without stopping AetherSora).
- **Open Config Folder** / **Exit AetherSora**.

### Mode Comparison

AetherSora enforces compile-time mutual exclusion (`enum ActiveTransport { WifiAdb, WifiApp, Bluetooth }`): **only one audio transport exists in memory at any moment**. In **Auto** mode, when running on Bluetooth fallback, a 0% CPU passive watcher listens on UDP `19480`/`19481` and mDNS `224.0.0.251:5353` so connecting the Companion App or re-enabling Wireless Debugging automatically drops Bluetooth and upgrades to 48 kHz Wi-Fi Lossless.

| Mode | Best For | Phone Setup | Audio Quality & Latency | PC Footprint |
| :--- | :--- | :--- | :--- | :--- |
| **Auto** (`--mode auto`) | Hands-free daily use | Auto-upgrades BT <-> Companion App <-> ADB Wireless | Best available (up to 48 kHz Lossless) | ~1.8-5.0 MB RAM, 0% CPU idle |
| **Wi-Fi Companion App** (`--mode wifi-app`) | Android 10+ without Dev Mode | 24.5 KB APK, 1-tap LAN discovery + auto-mute | 48 kHz 16-bit Stereo PCM (~15-30 ms) | ~1.77 MB RAM standby, < 0.5% CPU |
| **Wi-Fi ADB Wireless** (`--mode wifi-adb`) | Android 11+ power users | Wireless Debugging + one-time `--pair` (zero APK) | 48 kHz 16-bit Stereo PCM (~15-30 ms) | ~1.8-4.0 MB RAM, < 0.5% CPU |
| **Bluetooth A2DP Sink** (`--mode bt`) | Android & iOS zero-app setup | Windows Bluetooth pairing only | Windows SBC Stereo (44.1 kHz, ~150-250 ms) | ~4.2-4.6 MB RAM, 0% CPU idle |

---

## Troubleshooting

### Companion App does not discover the PC on LAN
- Ensure AetherSora PC is in **Auto** or **Wi-Fi 5GHz (Companion App)** mode.
- Allow `AetherSora.exe` through **Windows Defender Firewall** for UDP ports `19480` and `19481` (especially if your Windows Wi-Fi profile is set to **Public**). You can also enter the PC's Wi-Fi IP manually in the app.

### Companion App produces silence when Auto-mute is enabled
- Most newer Android ROMs apply speaker volume at the Audio HAL after `AudioPlaybackCapture`, allowing silence on the phone while streaming full audio to the PC. If your specific ROM silences capture at volume `0`, uncheck **Auto-mute phone speaker** in the app or use **ADB Wireless** mode (`REMOTE_SUBMIX`).

### `adb pair` fails with `error: protocol fault`
- Double-check the IP address and use the **pairing port inside the pairing popup** (not the outer Wireless Debugging port), keeping the popup open until pairing finishes.

### Connected in Wi-Fi ADB mode, but no sound plays on the PC
- Disconnect Bluetooth audio on your phone so Android routes media to `REMOTE_SUBMIX` instead of Bluetooth A2DP.
- Verify Windows Defender Firewall allows inbound UDP `19480` and turn up phone media volume.
- On Xiaomi HyperOS / MIUI, enable **USB debugging (Security settings)** in Developer options if `AudioRecord` initialization is blocked.

### Audio stutters in Bluetooth mode
- Disable **"Allow the computer to turn off this device to save power"** on your Bluetooth adapter in Device Manager and disable **USB selective suspend** in Windows Power Options.
- On combo Wi-Fi/Bluetooth cards with BLE peripherals, use **Wi-Fi 5 GHz mode** (`wifi-app` or `wifi-adb`) to avoid 2.4 GHz radio contention completely.

---

## CLI Reference & Configuration

CLI flags attach to your terminal when invoked from PowerShell or CMD (`| Out-Host` if you want PowerShell to wait for output):

```powershell
.\AetherSora.exe                               # Start in System Tray (default)
.\AetherSora.exe --stop                        # Cleanly stop a running tray instance
.\AetherSora.exe --console                     # Attach and keep console output open
.\AetherSora.exe --mode auto|wifi-app|wifi-adb|bt
.\AetherSora.exe --pair 192.168.1.50:41235 123456
.\AetherSora.exe --adb-host 192.168.1.50:37329
.\AetherSora.exe --buffer-ms 20                # Wi-Fi WASAPI jitter buffer (10..80 ms)
.\AetherSora.exe --scan                        # Interactive Bluetooth scan in console
.\AetherSora.exe --list                        # List paired Bluetooth audio sources
.\AetherSora.exe --device "Phone"              # Select Bluetooth device by name/ID
.\AetherSora.exe --no-reconnect                # Disable Bluetooth auto-reconnect watcher
.\AetherSora.exe --help
```

### Configuration (`device_id.cfg`)
Stored locally next to `AetherSora.exe` (excluded from Git via `.gitignore`):
1. Cached Bluetooth `DeviceId`
2. Cached Bluetooth display name
3. Cached Android Wireless Debugging IP / `IP:Port`
4. Last selected transport mode (`auto` / `wifi-app` / `wifi-adb` / `bt`)

---

## Project Structure

```text
AetherSora/
├── android/
│   ├── agent/          # 5.3 KB ADB DEX agent (REMOTE_SUBMIX, embedded in AetherSora.exe)
│   └── companion/      # 24.5 KB pure-SDK Android 10+ Companion APK (AudioPlaybackCapture)
├── assets/             # Chrono-Star SVG logos and Windows PE icon resource
├── src/
│   ├── bluetooth/      # Isolated WinRT AudioPlaybackConnection receiver
│   ├── wifi/           # UDP framing, SPSC ring buffer, WASAPI renderer, LAN discovery, mDNS watch, ADB
│   ├── config.rs       # Local device_id.cfg persistence
│   ├── log.rs          # In-memory 250-line ring log buffer
│   ├── main.rs         # Native GUI entry point, CLI parser, ActiveTransport state machine
│   ├── signal.rs       # AppSignal, single-instance guard, named --stop event, EcoQoS opt-out
│   └── tray.rs         # Pure Win32 System Tray, Toast notifications, BT submenu, Debug Log viewer
├── build.rs            # Embeds Windows PE icon resource
└── Cargo.toml          # Zero third-party Rust crates beyond windows = "0.58.0"
```

---

## License

This project is licensed under the [MIT License](LICENSE).
