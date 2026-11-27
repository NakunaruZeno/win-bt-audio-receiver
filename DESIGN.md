# Design System — AetherSora (Celestial Sky Minimalist)

Unified design specification shared across **Windows Tray Flyout**, **Android Companion (View System / Edge-to-Edge)**, and **iOS Companion (SwiftUI / HIG)** under the **Operate** product surface mode.

## 1. Visual Direction & Palette

No violet or purple tones appear anywhere in surfaces, gradients, borders, or glow rings. Elevation is communicated through clean surface steps and crisp `1dp`/`1pt` borders rather than heavy drop shadows.

| Role | Hex | Usage | Contrast vs Canvas (`#0B132B`) |
|---|---|---|---|
| Sky Canvas Top | `#091026` | Top backdrop edge gradient | Base |
| Sky Canvas Base | `#0D1633` | Primary backdrop / launch screen | Base |
| Surface Card | `#121E42` | Elevated list/setting card fill | Surface step |
| Surface Selected | `#182957` | Selected PC card fill | Surface step |
| Surface Input | `#0E1838` | Manual IP input well | Recessed step |
| Border Subtle | `#22366E` | 1dp/1pt resting card/input hairline | Structural |
| Border Active | `#38BDF8` | 1.5dp/1.5pt selected PC / focus ring | Accent |
| Divider Line | `#1B2C5C` | Subtle hairline separators | Structural |
| Accent Sky Azure | `#38BDF8` | Primary CTA fill, scan sweep, focus tint | 9.5:1 |
| Accent Starlight Gold | `#FACC15` | Active streaming / Disconnect CTA | 12.1:1 |
| Status Live | `#34D399` | Active streaming semantic indicator dot | 10.6:1 |
| Status Error | `#FB7185` | Validation / error indicator dot | 6.8:1 |
| Text Primary | `#F8FAFC` | Titles, PC hostnames, primary labels | 16.5:1 |
| Text Secondary | `#94B3D8` | Sky-tinted subtitles & metadata | 8.4:1 |
| Text Tertiary | `#7390BA` | Quiet helper hints & technical footers | 5.4:1 |
| Text On Accent | `#081126` | Text & icons inside Sky / Gold CTA pills | 10.8:1 (Sky) / 13.7:1 (Gold) |

## 2. Typography

- **Android:** System `Roboto` (`sans-serif`, `sans-serif-medium`) + `monospace` for IP/port values, sized strictly in `sp` for system font scaling.
- **iOS:** System `SF Pro` using semantic Dynamic Type styles (`.headline`, `.title3`, `.subheadline`, `.footnote`, `.caption2`) and `.monospaced()` digits for IP/port strings. Floor `>= 11pt`.
- **Hierarchy:** Screen Title `20sp`/`.title3` semibold (`#F8FAFC`), Status Headline `18sp`/`.headline` semibold, Card Title `15sp`/`.subheadline` medium, Section Label `13sp`/`.footnote` medium (`#94B3D8`), Technical Metadata `12sp`/`.caption` monospace.

## 3. Shape, Spacing & Touch Targets

- **8dp/8pt Rhythm Grid:** `4` tight icon gap, `8` card stack spacing, `12` section title gap, `16` card inner padding & outer horizontal margin (`20` safe breathing margin), `24` hero block clearance.
- **Radii:** Cards `16dp`/`16pt`, Inputs `14dp`/`14pt`, Status/Format badges `12dp`/`12pt`, Primary Action CTA Full Pill (`28dp` / `Capsule`).
- **Minimum Touch Targets:** `48 × 48 dp` on Android (`>= 8dp` gap between controls); `44 × 44 pt` on iOS (`>= 8pt` spacing), with native `.safeAreaInset(edge: .bottom)` / `WindowInsets` clearance so interactive controls never collide with notches, Dynamic Island, status bars, or home gesture bars.
- **Icons:** Strictly crisp vector icons (`20dp–24dp`, `1.75` stroke weight on Android; monochrome/hierarchical **SF Symbols** on iOS). Zero emojis (`📱`, `📶`, `⚠️`) as structural UI icons.

## 4. Layout Structure (Minimalist 1-Screen Operate Flow)

1. **Top Bar:** Brand mark + `"AetherSora"` wordmark left-aligned, `"48 kHz PCM"` quiet bordered pill badge right-aligned.
2. **Hero Signature Stage:** Centered `96dp/96pt` **Celestial Chrono-Star Audio Orb**, concise 1-line status title, and 1-line semantic status dot + subtitle.
3. **Nearby PCs Section:** `"Nearby PCs"` label + tonal `"Scan"` pill button (`>= 48dp` / `>= 44pt`), vertical stack of selectable PC cards (desktop icon, hostname, `ip:port` in monospace, check indicator when active), and a calm actionable empty-state card when no receiver responds.
4. **Collapsible Manual IP Disclosure:** Interactive `"Enter IP manually"` row with rotating chevron (`180ms` ease). Automatically **collapsed** when one or more LAN PCs are discovered; automatically **expanded** when 0 PCs are found or manually opened.
5. **Platform Audio Row:**
   - **Android:** Elevated card with speaker-mute vector icon, two-line copy (`"Auto-mute phone speaker"` / `"Restores volume on disconnect"`), and Sky-themed native `Switch`.
   - **iOS:** Calm tip card (`speaker.slash` SF Symbol) guiding the user to set iPhone hardware volume to `0` for zero echo while ReplayKit streams full audio to PC.
6. **Pinned Bottom CTA Dock:** Full-width `56dp/56pt` pill CTA (`#38BDF8` `"Connect & Stream"` when idle -> `#FACC15` `"Disconnect"` while streaming).

## 5. Signature Interaction & Motion

- **Signature Move — Celestial Chrono-Star Audio Orb:** Frames the Chrono-Star symbol (`assets/logo.svg`) inside concentric orbital hairlines with four subtle starlight pinpoints.
  - **Scanning:** Smooth circular `90°` Sky Azure orbital sweep arc.
  - **Streaming:** Real-time PCM RMS level drives an exponential-smoothed breathing halo ring (`#38BDF8` -> `#34D399`) and outer ring expansion.
- **Motion Discipline:** `150–250ms` state transitions only; zero ambient CPU loops when idle (`Choreographer`/CADisplayLink stops completely in idle); respects Android `ANIMATOR_DURATION_SCALE == 0` and iOS `@Environment(\.accessibilityReduceMotion)` cleanly by replacing motion with static high-contrast ring states.
