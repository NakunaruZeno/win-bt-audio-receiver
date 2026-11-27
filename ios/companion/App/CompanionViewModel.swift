import Combine
import Foundation
import SwiftUI
import UIKit

/**
 * View model driving LAN PC discovery, AppConfigServer IPC telemetry, manual IP persistence,
 * and ReplayKit screen-capture state synchronization.
 */
@MainActor
final class CompanionViewModel: ObservableObject {
    private static let lastIpKey = "aethersora_last_pc_ip"

    @Published var discoveredPCs: [DiscoveredPC] = []
    @Published var manualIp: String {
        didSet {
            UserDefaults.standard.set(manualIp, forKey: Self.lastIpKey)
            pushTargetToBridge()
        }
    }
    @Published var isManualIpExpanded = false
    @Published var isScanning = false
    @Published var isStreaming = false
    @Published var isErrorState = false
    @Published var audioLevel: Float = 0
    @Published var statusTitle = "Ready to stream"
    @Published var statusSubtitle = "Scanning Wi-Fi LAN for PCs..."

    private var userToggledManualIp = false
    private let bridgeServer = AppConfigServer()
    private var captureObserver: NSObjectProtocol?

    init() {
        let saved = UserDefaults.standard.string(forKey: Self.lastIpKey) ?? ""
        self.manualIp = saved
        bridgeServer.updateTarget(ip: saved, port: currentTargetPort())
    }

    var trimmedIp: String {
        manualIp.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    var isTargetValid: Bool {
        Self.isPlausibleIpv4(trimmedIp)
    }

    var orbState: CelestialOrbView.OrbState {
        if isStreaming {
            return .streaming
        }
        if isScanning {
            return .scanning
        }
        return .idle
    }

    func onAppear() {
        startBridgeServer()
        observeSystemCaptureChanges()
        if discoveredPCs.isEmpty {
            scanLan()
        }
        refreshCaptureFallback()
    }

    func onSceneActive() {
        startBridgeServer()
        pushTargetToBridge()
        refreshCaptureFallback()
    }

    func scanLan() {
        guard !isScanning else { return }
        isScanning = true
        isErrorState = false
        if !isStreaming {
            statusTitle = "Searching LAN"
            statusSubtitle = "Listening for AetherSora PC on Wi-Fi..."
        }

        Task {
            let found = await Task.detached(priority: .userInitiated) {
                LanDiscovery.scan(timeoutMs: 1200)
            }.value

            self.isScanning = false
            self.discoveredPCs = found

            if !found.isEmpty {
                if self.trimmedIp.isEmpty {
                    self.manualIp = found[0].ip
                }
                if !self.userToggledManualIp {
                    self.isManualIpExpanded = false
                }
            } else if !self.userToggledManualIp {
                self.isManualIpExpanded = true
            }

            if !self.isStreaming {
                if let active = self.selectedOrFirstPc() {
                    self.statusTitle = "Ready to stream"
                    self.statusSubtitle = "Target: \(active.hostname) • \(active.ip)"
                } else if !self.trimmedIp.isEmpty {
                    self.statusTitle = "Saved PC ready"
                    self.statusSubtitle = "Will send audio to \(self.trimmedIp)"
                } else {
                    self.statusTitle = "No PC discovered"
                    self.statusSubtitle = "Enter your PC's Wi-Fi IP below"
                }
            }
        }
    }

    func selectPC(_ pc: DiscoveredPC) {
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        manualIp = pc.ip
        isErrorState = false
        if !isStreaming {
            statusTitle = "Ready to stream"
            statusSubtitle = "Target: \(pc.hostname) • \(pc.ip)"
        }
    }

    func toggleManualIp() {
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        userToggledManualIp = true
        isManualIpExpanded.toggle()
    }

    func prepareTargetBeforeBroadcast() {
        pushTargetToBridge()
    }

    func handleMissingTargetTap() {
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        userToggledManualIp = true
        isManualIpExpanded = true
        isErrorState = true
        statusTitle = "PC IP required"
        statusSubtitle = "Tap a discovered PC or enter a valid IPv4 address"
    }

    func pushTargetToBridge() {
        bridgeServer.updateTarget(ip: trimmedIp, port: currentTargetPort())
    }

    private func currentTargetPort() -> UInt16 {
        let ip = manualIp.trimmingCharacters(in: .whitespacesAndNewlines)
        return discoveredPCs.first(where: { $0.ip == ip })?.audioPort
            ?? AetherProtocol.defaultAudioPort
    }

    private func selectedOrFirstPc() -> DiscoveredPC? {
        guard !discoveredPCs.isEmpty else { return nil }
        return discoveredPCs.first(where: { $0.ip == trimmedIp }) ?? discoveredPCs[0]
    }

    private func startBridgeServer() {
        bridgeServer.start { [weak self] telemetry in
            Task { @MainActor in
                self?.applyTelemetry(telemetry)
            }
        }
    }

    private func applyTelemetry(_ telemetry: ExtensionTelemetry) {
        switch telemetry.state {
        case .streaming:
            isStreaming = true
            isErrorState = false
            audioLevel = telemetry.rmsLevel
            if telemetry.acked {
                statusTitle = "Streaming Lossless"
                statusSubtitle = "48 kHz PCM -> \(telemetry.pcIp)"
            } else {
                statusTitle = "Streaming (Unacked)"
                statusSubtitle = "No ACK from \(telemetry.pcIp) — check PC Firewall"
            }
        case .stopped:
            isStreaming = false
            isErrorState = false
            audioLevel = 0
            statusTitle = "Ready to stream"
            statusSubtitle = trimmedIp.isEmpty ? "Stream stopped" : "Target: \(trimmedIp)"
        case .error:
            isStreaming = false
            isErrorState = true
            audioLevel = 0
            statusTitle = "Stream error"
            statusSubtitle = telemetry.detail.isEmpty ? "Could not reach AetherSora PC" : telemetry.detail
        }
    }

    private func observeSystemCaptureChanges() {
        guard captureObserver == nil else { return }
        captureObserver = NotificationCenter.default.addObserver(
            forName: UIScreen.capturedDidChangeNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            Task { @MainActor in
                self?.refreshCaptureFallback()
            }
        }
    }

    private func refreshCaptureFallback() {
        let captured = (UIApplication.shared.connectedScenes.first as? UIWindowScene)?
            .screen.isCaptured ?? false
        if !captured && isStreaming {
            isStreaming = false
            audioLevel = 0
            statusTitle = "Ready to stream"
            statusSubtitle = trimmedIp.isEmpty ? "Stream stopped" : "Target: \(trimmedIp)"
        }
    }

    static func isPlausibleIpv4(_ ip: String) -> Bool {
        guard !ip.isEmpty else { return false }
        let parts = ip.split(separator: ".", omittingEmptySubsequences: false)
        guard parts.count == 4 else { return false }
        for part in parts {
            guard !part.isEmpty, part.count <= 3, let val = Int(part), val >= 0, val <= 255 else {
                return false
            }
        }
        return true
    }
}
