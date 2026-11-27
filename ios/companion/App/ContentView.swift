import SwiftUI

/**
 * Minimalist 1-screen Celestial Sky UI matching AetherSora desktop and Android companions.
 */
struct ContentView: View {
    @ObservedObject var viewModel: CompanionViewModel
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.scenePhase) private var scenePhase
    @FocusState private var ipFieldFocused: Bool

    var body: some View {
        ZStack {
            LinearGradient(
                colors: [CelestialTheme.bgTop, CelestialTheme.bgBase, CelestialTheme.bgBottom],
                startPoint: .top,
                endPoint: .bottom
            )
            .ignoresSafeArea()
            .onTapGesture {
                ipFieldFocused = false
            }

            VStack(spacing: 0) {
                headerBar
                    .padding(.horizontal, 20)
                    .padding(.top, 8)
                    .padding(.bottom, 8)

                ScrollView(showsIndicators: false) {
                    VStack(alignment: .leading, spacing: 0) {
                        heroStage
                            .padding(.top, 10)
                            .padding(.bottom, 22)

                        nearbyHeader
                            .padding(.bottom, 8)

                        nearbyPcList

                        manualIpDisclosure
                            .padding(.top, 10)

                        zeroEchoTipCard
                            .padding(.top, 14)
                    }
                    .padding(.horizontal, 20)
                    .padding(.bottom, 20)
                }
            }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            bottomCtaDock
        }
        .toolbar {
            ToolbarItemGroup(placement: .keyboard) {
                Spacer()
                Button("Done") {
                    ipFieldFocused = false
                }
                .foregroundStyle(CelestialTheme.accentSky)
            }
        }
        .onAppear {
            viewModel.onAppear()
        }
        .onChange(of: scenePhase) { phase in
            if phase == .active {
                viewModel.onSceneActive()
            }
        }
    }

    private var headerBar: some View {
        HStack(spacing: 12) {
            Text("AetherSora")
                .font(.title3.weight(.semibold))
                .foregroundStyle(CelestialTheme.textPrimary)

            Spacer(minLength: 8)

            Text("48 kHz PCM")
                .font(.system(.caption2, design: .monospaced).weight(.medium))
                .foregroundStyle(CelestialTheme.textSecondary)
                .padding(.horizontal, 10)
                .padding(.vertical, 5)
                .background(
                    RoundedRectangle(cornerRadius: CelestialTheme.badgeRadius, style: .continuous)
                        .fill(Color(hex: 0x132147))
                )
                .overlay(
                    RoundedRectangle(cornerRadius: CelestialTheme.badgeRadius, style: .continuous)
                        .stroke(Color(hex: 0x233A74), lineWidth: 1)
                )
        }
    }

    private var heroStage: some View {
        VStack(spacing: 0) {
            CelestialOrbView(state: viewModel.orbState, audioLevel: viewModel.audioLevel)

            Text(viewModel.statusTitle)
                .font(.headline.weight(.semibold))
                .foregroundStyle(CelestialTheme.textPrimary)
                .multilineTextAlignment(.center)
                .padding(.top, 12)

            HStack(spacing: 8) {
                Circle()
                    .fill(statusDotColor)
                    .frame(width: 8, height: 8)

                Text(viewModel.statusSubtitle)
                    .font(.footnote)
                    .foregroundStyle(CelestialTheme.textSecondary)
                    .lineLimit(2)
                    .multilineTextAlignment(.center)
            }
            .padding(.top, 6)
        }
        .frame(maxWidth: .infinity)
    }

    private var statusDotColor: Color {
        if viewModel.isStreaming {
            return CelestialTheme.statusLive
        }
        if viewModel.isErrorState {
            return CelestialTheme.statusError
        }
        return CelestialTheme.accentSky
    }

    private var nearbyHeader: some View {
        HStack {
            Text("Nearby PCs")
                .font(.footnote.weight(.medium))
                .foregroundStyle(CelestialTheme.textSecondary)

            Spacer()

            Button {
                viewModel.scanLan()
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: "arrow.clockwise")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(CelestialTheme.accentSky)
                    Text(viewModel.isScanning ? "Scanning" : "Scan")
                        .font(.footnote.weight(.medium))
                        .foregroundStyle(CelestialTheme.textPrimary)
                }
                .padding(.horizontal, 14)
                .frame(minHeight: 38)
                .background(
                    Capsule()
                        .fill(CelestialTheme.surfaceTonal)
                )
                .overlay(
                    Capsule()
                        .stroke(CelestialTheme.borderTonal, lineWidth: 1)
                )
            }
            .frame(minHeight: 44)
            .buttonStyle(.plain)
            .disabled(viewModel.isScanning)
            .opacity(viewModel.isScanning ? 0.65 : 1.0)
            .accessibilityLabel("Scan local Wi-Fi network for AetherSora PCs")
        }
    }

    @ViewBuilder
    private var nearbyPcList: some View {
        if viewModel.discoveredPCs.isEmpty {
            HStack(spacing: 14) {
                Image(systemName: "desktopcomputer")
                    .font(.title3)
                    .foregroundStyle(CelestialTheme.textSecondary.opacity(0.75))

                VStack(alignment: .leading, spacing: 3) {
                    Text("No PC found on Wi-Fi")
                        .font(.subheadline.weight(.medium))
                        .foregroundStyle(CelestialTheme.textPrimary)

                    Text("Open AetherSora on PC in Auto or Wi-Fi Companion mode on the same 5 GHz LAN, or enter IP below.")
                        .font(.caption)
                        .foregroundStyle(CelestialTheme.textSecondary)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(16)
            .background(
                RoundedRectangle(cornerRadius: CelestialTheme.cardRadius, style: .continuous)
                    .fill(CelestialTheme.surfaceCard)
            )
            .overlay(
                RoundedRectangle(cornerRadius: CelestialTheme.cardRadius, style: .continuous)
                    .stroke(CelestialTheme.borderSubtle, lineWidth: 1)
            )
        } else {
            VStack(spacing: 8) {
                ForEach(viewModel.discoveredPCs) { pc in
                    let isSelected = viewModel.trimmedIp == pc.ip
                    Button {
                        viewModel.selectPC(pc)
                    } label: {
                        HStack(spacing: 14) {
                            Image(systemName: "desktopcomputer")
                                .font(.title3)
                                .foregroundStyle(
                                    isSelected ? CelestialTheme.accentSky : CelestialTheme.textSecondary
                                )

                            VStack(alignment: .leading, spacing: 2) {
                                Text(pc.hostname)
                                    .font(.subheadline.weight(.medium))
                                    .foregroundStyle(CelestialTheme.textPrimary)
                                    .lineLimit(1)

                                Text("\(pc.ip):\(pc.audioPort)")
                                    .font(.system(.caption, design: .monospaced))
                                    .foregroundStyle(CelestialTheme.textSecondary)
                                    .lineLimit(1)
                            }

                            Spacer(minLength: 8)

                            if isSelected {
                                Image(systemName: "checkmark.circle.fill")
                                    .font(.title3)
                                    .foregroundStyle(CelestialTheme.accentSky)
                            }
                        }
                        .padding(.horizontal, 16)
                        .padding(.vertical, 12)
                        .frame(minHeight: 62)
                        .background(
                            RoundedRectangle(cornerRadius: CelestialTheme.cardRadius, style: .continuous)
                                .fill(isSelected ? CelestialTheme.surfaceSelected : CelestialTheme.surfaceCard)
                        )
                        .overlay(
                            RoundedRectangle(cornerRadius: CelestialTheme.cardRadius, style: .continuous)
                                .stroke(
                                    isSelected ? CelestialTheme.borderActive : CelestialTheme.borderSubtle,
                                    lineWidth: isSelected ? 1.5 : 1
                                )
                        )
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(isSelected ? [.isSelected] : [])
                }
            }
        }
    }

    private var manualIpDisclosure: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button {
                if reduceMotion {
                    viewModel.toggleManualIp()
                } else {
                    withAnimation(.easeOut(duration: 0.18)) {
                        viewModel.toggleManualIp()
                    }
                }
            } label: {
                HStack {
                    Text("Enter IP manually")
                        .font(.footnote.weight(.medium))
                        .foregroundStyle(CelestialTheme.textSecondary)

                    Spacer()

                    Image(systemName: "chevron.down")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(CelestialTheme.textSecondary)
                        .rotationEffect(.degrees(viewModel.isManualIpExpanded ? 180 : 0))
                }
                .frame(minHeight: 44)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            if viewModel.isManualIpExpanded {
                TextField("192.168.1.50", text: $viewModel.manualIp)
                    .focused($ipFieldFocused)
                    .keyboardType(.numbersAndPunctuation)
                    .textInputAutocapitalization(.never)
                    .disableAutocorrection(true)
                    .font(.system(.subheadline, design: .monospaced))
                    .foregroundStyle(CelestialTheme.textPrimary)
                    .padding(.horizontal, 16)
                    .frame(height: 50)
                    .background(
                        RoundedRectangle(cornerRadius: CelestialTheme.inputRadius, style: .continuous)
                            .fill(CelestialTheme.surfaceInput)
                    )
                    .overlay(
                        RoundedRectangle(cornerRadius: CelestialTheme.inputRadius, style: .continuous)
                            .stroke(
                                ipFieldFocused ? CelestialTheme.borderActive : CelestialTheme.borderSubtle,
                                lineWidth: ipFieldFocused ? 1.5 : 1
                            )
                    )
            }
        }
    }

    private var zeroEchoTipCard: some View {
        HStack(spacing: 14) {
            Image(systemName: "speaker.slash")
                .font(.title3)
                .foregroundStyle(CelestialTheme.accentSky)

            VStack(alignment: .leading, spacing: 2) {
                Text("Zero-echo tip for iPhone")
                    .font(.subheadline.weight(.medium))
                    .foregroundStyle(CelestialTheme.textPrimary)

                Text("Lower volume to 0 so only PC speakers play; ReplayKit still streams 100% audio.")
                    .font(.caption)
                    .foregroundStyle(CelestialTheme.textSecondary)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(16)
        .background(
            RoundedRectangle(cornerRadius: CelestialTheme.cardRadius, style: .continuous)
                .fill(CelestialTheme.surfaceCard)
        )
        .overlay(
            RoundedRectangle(cornerRadius: CelestialTheme.cardRadius, style: .continuous)
                .stroke(CelestialTheme.borderSubtle, lineWidth: 1)
        )
    }

    private var bottomCtaDock: some View {
        VStack(spacing: 8) {
            ZStack {
                HStack(spacing: 10) {
                    Image(systemName: viewModel.isStreaming ? "stop.fill" : "waveform")
                        .font(.body.weight(.semibold))
                    Text(viewModel.isStreaming ? "Disconnect" : "Connect & Stream")
                        .font(.body.weight(.semibold))
                }
                .foregroundStyle(CelestialTheme.textOnAccent)
                .frame(maxWidth: .infinity)
                .frame(height: 54)
                .background(
                    Capsule()
                        .fill(viewModel.isStreaming ? CelestialTheme.accentGold : CelestialTheme.accentSky)
                )

                if viewModel.isTargetValid {
                    BroadcastPickerButton {
                        ipFieldFocused = false
                        viewModel.prepareTargetBeforeBroadcast()
                    }
                    .frame(maxWidth: .infinity)
                    .frame(height: 54)
                    .opacity(0.015)
                    .accessibilityLabel(
                        viewModel.isStreaming
                            ? "Stop AetherSora audio broadcast"
                            : "Start AetherSora audio broadcast"
                    )
                } else {
                    Button {
                        viewModel.handleMissingTargetTap()
                        ipFieldFocused = true
                    } label: {
                        Color.clear
                            .frame(maxWidth: .infinity)
                            .frame(height: 54)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Connect and stream requires a valid PC IP")
                }
            }

            Text("Tap Start Broadcast to route system audio • DRM apps muted by iOS")
                .font(.caption2)
                .foregroundStyle(CelestialTheme.textTertiary)
                .multilineTextAlignment(.center)
        }
        .padding(.horizontal, 20)
        .padding(.top, 10)
        .padding(.bottom, 12)
        .background(CelestialTheme.bgBottom.opacity(0.96))
    }
}
