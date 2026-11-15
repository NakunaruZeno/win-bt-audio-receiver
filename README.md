# AetherSora: Windows Bluetooth Audio Receiver (A2DP Sink)

An ultra-lightweight, zero-overhead Windows console application written in Rust that enables remote audio playback (Bluetooth A2DP Sink). AetherSora streams audio from Bluetooth-paired source devices (such as smartphones and tablets) directly to your Windows PC speakers without requiring third-party drivers or external runtimes.

---

## Overview

Modern Windows builds (Windows 10 Version 2004 build 19041 and newer, including Windows 11) provide native operating system kernel support for Bluetooth A2DP Sink routing via the Windows Runtime (WinRT) `AudioPlaybackConnection` API. 

Because Windows natively handles Bluetooth transport, audio codec decoding (such as SBC and AAC), and mixer integration, this application does not execute heavy decoding algorithms. Instead, it serves as a lightweight control plane calling native Windows APIs directly through Microsoft's official `windows` crate.

---

## Key Features

- Minimal Resource Consumption: Binary footprint of approximately 190 KB, utilizing 0% CPU when streaming and negligible system memory.
- Zero External Dependencies: Built strictly using Microsoft's official `windows-rs` projection, without GUI frameworks or third-party audio libraries.
- Fast Connect: Automatically caches target device identifiers (`device_id.cfg`) to bypass full device discovery on subsequent launches, establishing connections in under one second.
- Automatic Reconnection: Implements `DeviceWatcher` along with connection state monitoring (`StateChanged`) to detect when a disconnected device re-enters range, automatically re-establishing the audio stream.
- Graceful Signal Handling: Intercepts console termination events (`Ctrl+C`, window closing, logoff, system shutdown) to close WinRT handles cleanly before process exit.

---

## System Requirements

- Operating System: Windows 10 Version 2004 (Build 19041) or higher, or Windows 11.
- Bluetooth Adapter: Bluetooth adapter supporting A2DP Sink profiles.
- Pre-requisite: The remote audio device (e.g., smartphone) must already be paired with the host PC through Windows Settings.
- Build Toolchain (Source Builds): Rust compiler (`rustc` and `cargo` via `rustup`) with the MSVC toolchain (`x86_64-pc-windows-msvc`).

---

## Building from Source

Ensure the Rust toolchain is installed on the host machine:

```powershell
# Install Rustup if not already installed
winget install Rustlang.Rustup
```

Clone the repository and compile with release optimizations:

```powershell
cargo build --release
```

The resulting optimized executable will be located at:
```text
target\release\aethersora.exe
```

---

## Usage

Run the compiled executable directly from PowerShell or Command Prompt:

```powershell
# Default launch: Uses cached device ID if available; otherwise scans paired devices
.\aethersora.exe

# Force a scan of paired devices and update the cached configuration
.\aethersora.exe --scan

# List all paired Bluetooth audio source devices and exit
.\aethersora.exe --list

# Connect directly to a device matching a name or ID substring
.\aethersora.exe --device "Phone"

# Display CLI help
.\aethersora.exe --help
```

To stop the receiver and cleanly release the audio endpoint, press `Ctrl+C` in the console window.

---

## Architecture and Workflow

1. Device Enumeration: Uses `AudioPlaybackConnection::GetDeviceSelector()` with `DeviceInformation::FindAllAsyncAqsFilter()` to retrieve paired A2DP audio source devices.
2. Connection Initialization: Invokes `AudioPlaybackConnection::TryCreateFromId()` with the target device ID.
3. Stream Activation: Calls `StartAsync()` to configure the local endpoint, followed by `OpenAsync()` to open the active audio stream.
4. Background Management: Attaches event listeners to `DeviceWatcher` (`Added`, `Updated`) and `AudioPlaybackConnection` (`StateChanged`). The main thread remains idle waiting on a condition variable until signaled to reconnect or terminate.
5. Resource Disposal: Calls `AudioPlaybackConnection::Close()` to release system audio handles upon shutdown.

---

## Configuration

When a device is selected, the application generates a local configuration file named `device_id.cfg` in the same directory as the executable. This file stores:

1. Line 1: Target Device ID string.
2. Line 2: Device display name.

This file is automatically managed and excluded from version control. Delete this file or run with `--scan` to configure a different device.

---

## License

This project is licensed under the MIT License.
