# Contributing to AetherSora

Thank you for your interest in contributing to AetherSora! We welcome bug reports, feature proposals, and pull requests from the community.

## Ground Rules

- Be respectful and considerate toward fellow contributors.
- Keep the codebase lightweight: Avoid introducing unnecessary third-party dependencies, heavy UI libraries, or excessive abstractions.
- All code must target Windows 10 Version 2004 (Build 19041) or higher, using the official Microsoft `windows` crate.

## How Can I Contribute?

### Reporting Bugs

Before creating a bug report, please check existing issues to ensure the problem has not already been reported.

When creating an issue, provide:
1. Windows version and OS build number (`winver`).
2. Bluetooth adapter model and driver version.
3. Audio source device model and operating system (e.g., iPhone iOS 18, Pixel Android 15).
4. Clear reproduction steps and console output/error messages.

### Suggesting Enhancements

Feature requests are welcome! Please open an issue describing:
1. The problem or limitation you want solved.
2. The proposed solution or behavior.
3. Any relevant Windows API considerations.

### Submitting Pull Requests

1. Fork the repository and create your feature branch:
   ```powershell
   git checkout -b feature/your-feature-name
   ```
2. Write clean, idiomatic Rust code:
   - Format your code using `cargo fmt`.
   - Ensure there are no warnings using `cargo check` and `cargo clippy`.
3. Test your changes locally on a real Windows machine with Bluetooth audio hardware.
4. Commit your changes with clear, descriptive commit messages.
5. Push to your fork and submit a Pull Request against the `main` branch.
