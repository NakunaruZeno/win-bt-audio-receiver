import CoreMedia
import Foundation
import ReplayKit

/**
 * ReplayKit Broadcast Upload Extension capturing iOS `.audioApp` system/app playback
 * and streaming 5 ms 48 kHz Lossless PCM UDP packets directly to AetherSora on Windows.
 */
final class SampleHandler: RPBroadcastSampleHandler {
    private let audioQueue = DispatchQueue(
        label: "com.aethersora.replaykit.audio",
        qos: .userInteractive)
    private let converter = AudioPcmConverter()
    private let bridgeClient = ExtensionConfigClient()

    private var sender: AetherUdpSender?
    private var targetIp: String = ""
    private var isAcked = false
    private var paused = false
    private var silentPacketCount = 0
    private var lastKeepaliveNs: UInt64 = 0
    private var lastTelemetryNs: UInt64 = 0

    override func broadcastStarted(withSetupInfo setupInfo: [String: NSObject]?) {
        audioQueue.sync {
            converter.reset()
            paused = false
            silentPacketCount = 0
            lastKeepaliveNs = 0
            lastTelemetryNs = 0

            let resolvedTarget: (ip: String, port: UInt16)?
            if let infoIp = setupInfo?["pc_ip"] as? String, !infoIp.isEmpty {
                let infoPort = (setupInfo?["pc_port"] as? NSNumber)?.uint16Value
                    ?? AetherProtocol.defaultAudioPort
                resolvedTarget = (infoIp, infoPort)
            } else {
                resolvedTarget = bridgeClient.resolveTargetPC()
            }

            guard let target = resolvedTarget,
                let udp = AetherUdpSender(host: target.ip, port: target.port)
            else {
                let err = NSError(
                    domain: "com.aethersora.companion",
                    code: 404,
                    userInfo: [
                        NSLocalizedDescriptionKey:
                            "No AetherSora PC found. Open AetherSora on Windows and select a PC first."
                    ])
                bridgeClient.sendTelemetry(
                    state: .error,
                    pcIp: "",
                    rmsLevel: 0,
                    acked: false,
                    detail: "No AetherSora PC found on Wi-Fi LAN")
                finishBroadcastWithError(err)
                return
            }

            targetIp = target.ip
            isAcked = udp.sendHelloAndWaitAck()
            sender = udp

            bridgeClient.sendTelemetry(
                state: .streaming,
                pcIp: targetIp,
                rmsLevel: 0,
                acked: isAcked,
                detail: isAcked
                    ? "48 kHz PCM -> \(targetIp)"
                    : "Streaming unacked -> \(targetIp)")
        }
    }

    override func broadcastPaused() {
        audioQueue.sync {
            paused = true
        }
    }

    override func broadcastResumed() {
        audioQueue.sync {
            paused = false
        }
    }

    override func broadcastFinished() {
        audioQueue.sync {
            sender?.close()
            sender = nil
            converter.reset()
            bridgeClient.sendTelemetry(
                state: .stopped,
                pcIp: targetIp,
                rmsLevel: 0,
                acked: false,
                detail: "Stream stopped")
            targetIp = ""
        }
    }

    override func processSampleBuffer(
        _ sampleBuffer: CMSampleBuffer,
        with sampleBufferType: RPSampleBufferType
    ) {
        guard sampleBufferType == .audioApp else {
            return
        }

        audioQueue.sync {
            guard !paused, let activeSender = sender else { return }
            autoreleasepool {
                converter.process(sampleBuffer: sampleBuffer) { packetPtr, byteCount, rms in
                    if rms == 0 {
                        silentPacketCount += 1
                    } else {
                        silentPacketCount = 0
                    }

                    let now = DispatchTime.now().uptimeNanoseconds
                    if silentPacketCount <= 50 {
                        activeSender.sendAudio(pcmBytes: packetPtr, byteCount: byteCount)
                        lastKeepaliveNs = now
                        maybePublishTelemetry(rms: rms, nowNs: now)
                    } else if now >= lastKeepaliveNs && now - lastKeepaliveNs >= 400_000_000 {
                        activeSender.sendKeepalive()
                        lastKeepaliveNs = now
                        maybePublishTelemetry(rms: 0, nowNs: now)
                    }
                }
            }
        }
    }

    private func maybePublishTelemetry(rms: Float, nowNs: UInt64) {
        guard nowNs >= lastTelemetryNs && nowNs - lastTelemetryNs >= 65_000_000 else { return }
        lastTelemetryNs = nowNs
        bridgeClient.sendTelemetry(
            state: .streaming,
            pcIp: targetIp,
            rmsLevel: rms,
            acked: isAcked,
            detail: isAcked ? "48 kHz PCM -> \(targetIp)" : "Streaming unacked -> \(targetIp)")
    }
}
