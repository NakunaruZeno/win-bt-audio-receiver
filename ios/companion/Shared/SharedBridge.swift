import Darwin
import Foundation

/**
 * Live telemetry reported from the ReplayKit Broadcast Extension back to the Main App UI.
 */
struct ExtensionTelemetry: Sendable {
    enum State: String, Sendable {
        case streaming
        case stopped
        case error
    }

    let state: State
    let pcIp: String
    let rmsLevel: Float
    let acked: Bool
    let detail: String
}

/**
 * Dual-path IPC bridge connecting Main App (`AppConfigServer`) and Broadcast Upload Extension
 * (`ExtensionConfigClient`). Uses both the App Group container when signed AND a local loopback
 * UDP channel (`127.0.0.1:19489`) so free-Apple-ID AltStore/SideStore/Sideloadly builds work
 * reliably even when Apple strips App Group entitlements.
 */
enum AetherBridgeConfig {
    static let appGroupID = "group.com.aethersora.companion"
    static let bridgePort: UInt16 = 19489
    static let configFileName = "aethersora_target.txt"

    static func appGroupConfigURL() -> URL? {
        FileManager.default
            .containerURL(forSecurityApplicationGroupIdentifier: appGroupID)?
            .appendingPathComponent(configFileName)
    }

    static func saveTargetToAppGroup(ip: String, port: UInt16) {
        guard let url = appGroupConfigURL() else { return }
        let payload = "\(ip)|\(port)"
        try? payload.write(to: url, atomically: true, encoding: .utf8)
    }

    static func loadTargetFromAppGroup() -> (String, UInt16)? {
        guard let url = appGroupConfigURL(),
            let raw = try? String(contentsOf: url, encoding: .utf8)
        else {
            return nil
        }
        let parts = raw.trimmingCharacters(in: .whitespacesAndNewlines).split(separator: "|")
        guard let ip = parts.first, !ip.isEmpty else { return nil }
        let port = (parts.count >= 2 ? UInt16(parts[1]) : nil) ?? AetherProtocol.defaultAudioPort
        return (String(ip), port)
    }
}

/**
 * Main-app loopback UDP server (`127.0.0.1:19489`) answering target PC queries from
 * `SampleHandler` and forwarding live streaming state/RMS telemetry to the SwiftUI layer.
 */
final class AppConfigServer: @unchecked Sendable {
    private let lock = NSLock()
    private var targetIp: String = ""
    private var targetPort: UInt16 = AetherProtocol.defaultAudioPort
    private var stopRequested = false
    private var socketFd: Int32 = -1
    private let queue = DispatchQueue(label: "com.aethersora.bridge.server", qos: .utility)

    func updateTarget(ip: String, port: UInt16 = AetherProtocol.defaultAudioPort) {
        let trimmed = ip.trimmingCharacters(in: .whitespacesAndNewlines)
        lock.lock()
        targetIp = trimmed
        targetPort = port
        lock.unlock()
        AetherBridgeConfig.saveTargetToAppGroup(ip: trimmed, port: port)
    }

    func start(onTelemetry: @escaping @Sendable (ExtensionTelemetry) -> Void) {
        lock.lock()
        if socketFd >= 0 {
            lock.unlock()
            return
        }
        stopRequested = false
        lock.unlock()

        queue.async { [weak self] in
            self?.runServerLoop(onTelemetry: onTelemetry)
        }
    }

    func stop() {
        lock.lock()
        stopRequested = true
        let fd = socketFd
        socketFd = -1
        lock.unlock()
        if fd >= 0 {
            Darwin.close(fd)
        }
    }

    private func currentTargetSnapshot() -> (String, UInt16) {
        lock.lock()
        let ip = targetIp
        let port = targetPort
        lock.unlock()
        return (ip, port)
    }

    private func isStopping() -> Bool {
        lock.lock()
        let stop = stopRequested
        lock.unlock()
        return stop
    }

    private func runServerLoop(onTelemetry: @escaping @Sendable (ExtensionTelemetry) -> Void) {
        let fd = Darwin.socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP)
        guard fd >= 0 else { return }

        var reuse: Int32 = 1
        _ = Darwin.setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &reuse, socklen_t(MemoryLayout<Int32>.size))

        var tv = timeval(tv_sec: 0, tv_usec: 250_000)
        _ = Darwin.setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))

        var bindAddr = sockaddr_in()
        bindAddr.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        bindAddr.sin_family = sa_family_t(AF_INET)
        bindAddr.sin_port = AetherBridgeConfig.bridgePort.bigEndian
        bindAddr.sin_addr = in_addr(s_addr: inet_addr("127.0.0.1"))

        let bound = withUnsafePointer(to: &bindAddr) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { saPtr in
                Darwin.bind(fd, saPtr, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        guard bound == 0 else {
            Darwin.close(fd)
            return
        }

        lock.lock()
        if stopRequested {
            lock.unlock()
            Darwin.close(fd)
            return
        }
        socketFd = fd
        lock.unlock()

        var buf = [UInt8](repeating: 0, count: 512)
        while !isStopping() {
            var srcAddr = sockaddr_in()
            var srcLen = socklen_t(MemoryLayout<sockaddr_in>.size)
            let n = buf.withUnsafeMutableBytes { raw -> ssize_t in
                guard let base = raw.baseAddress else { return -1 }
                return withUnsafeMutablePointer(to: &srcAddr) { ptr in
                    ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { saPtr in
                        Darwin.recvfrom(fd, base, 512, 0, saPtr, &srcLen)
                    }
                }
            }
            if n <= 0 {
                continue
            }

            guard let text = String(bytes: buf[0..<Int(n)], encoding: .utf8) else {
                continue
            }

            if text.hasPrefix("AETHER_CFG_REQ") {
                let (ip, port) = currentTargetSnapshot()
                let replyBytes = Array("AETHER_CFG|\(ip)|\(port)".utf8)
                withUnsafePointer(to: &srcAddr) { ptr in
                    ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { saPtr in
                        replyBytes.withUnsafeBytes { raw in
                            guard let base = raw.baseAddress else { return }
                            _ = Darwin.sendto(fd, base, replyBytes.count, 0, saPtr, srcLen)
                        }
                    }
                }
            } else if text.hasPrefix("AETHER_STAT|"), let telemetry = parseTelemetry(text) {
                DispatchQueue.main.async {
                    onTelemetry(telemetry)
                }
            }
        }
    }

    private func parseTelemetry(_ text: String) -> ExtensionTelemetry? {
        let parts = text.split(separator: "|", omittingEmptySubsequences: false)
        guard parts.count >= 5,
            let state = ExtensionTelemetry.State(rawValue: String(parts[1]))
        else {
            return nil
        }
        let ip = String(parts[2])
        let rms = Float(parts[3]) ?? 0
        let acked = parts[4] == "1"
        let detail = parts.count >= 6 ? String(parts[5]) : ""
        return ExtensionTelemetry(state: state, pcIp: ip, rmsLevel: rms, acked: acked, detail: detail)
    }
}

/**
 * Extension-side helper resolving target PC config and reporting streaming telemetry back to UI.
 */
final class ExtensionConfigClient {
    private var sockFd: Int32 = -1
    private var serverAddr = sockaddr_in()

    init() {
        serverAddr.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        serverAddr.sin_family = sa_family_t(AF_INET)
        serverAddr.sin_port = AetherBridgeConfig.bridgePort.bigEndian
        serverAddr.sin_addr = in_addr(s_addr: inet_addr("127.0.0.1"))

        let fd = Darwin.socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP)
        if fd >= 0 {
            var tv = timeval(tv_sec: 0, tv_usec: 180_000)
            _ = Darwin.setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
            var noSigPipe: Int32 = 1
            _ = Darwin.setsockopt(
                fd, SOL_SOCKET, SO_NOSIGPIPE, &noSigPipe, socklen_t(MemoryLayout<Int32>.size))
            sockFd = fd
        }
    }

    deinit {
        if sockFd >= 0 {
            Darwin.close(sockFd)
        }
    }

    func resolveTargetPC() -> (ip: String, port: UInt16)? {
        if let loopbackTarget = queryLoopbackBridge(), !loopbackTarget.0.isEmpty {
            return loopbackTarget
        }
        if let appGroupTarget = AetherBridgeConfig.loadTargetFromAppGroup(), !appGroupTarget.0.isEmpty {
            return appGroupTarget
        }
        if let discovered = LanDiscovery.scan(timeoutMs: 900).first {
            return (discovered.ip, discovered.audioPort)
        }
        return nil
    }

    func sendTelemetry(
        state: ExtensionTelemetry.State,
        pcIp: String,
        rmsLevel: Float,
        acked: Bool,
        detail: String = ""
    ) {
        guard sockFd >= 0 else { return }
        let ackFlag = acked ? "1" : "0"
        let cleanDetail = detail.replacingOccurrences(of: "|", with: " ")
        let msg = String(
            format: "AETHER_STAT|%@|%@|%.3f|%@|%@",
            locale: Locale(identifier: "en_US_POSIX"),
            state.rawValue,
            pcIp,
            rmsLevel,
            ackFlag,
            cleanDetail)
        let bytes = Array(msg.utf8)
        var addrCopy = serverAddr
        withUnsafePointer(to: &addrCopy) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { saPtr in
                bytes.withUnsafeBytes { raw in
                    guard let base = raw.baseAddress else { return }
                    _ = Darwin.sendto(
                        sockFd, base, bytes.count, 0, saPtr, socklen_t(MemoryLayout<sockaddr_in>.size))
                }
            }
        }
    }

    private func queryLoopbackBridge() -> (String, UInt16)? {
        guard sockFd >= 0 else { return nil }
        let req = Array("AETHER_CFG_REQ".utf8)
        var addrCopy = serverAddr
        withUnsafePointer(to: &addrCopy) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { saPtr in
                req.withUnsafeBytes { raw in
                    guard let base = raw.baseAddress else { return }
                    _ = Darwin.sendto(
                        sockFd, base, req.count, 0, saPtr, socklen_t(MemoryLayout<sockaddr_in>.size))
                }
            }
        }

        var buf = [UInt8](repeating: 0, count: 256)
        let n = buf.withUnsafeMutableBytes { raw -> ssize_t in
            guard let base = raw.baseAddress else { return -1 }
            return Darwin.recvfrom(sockFd, base, 256, 0, nil, nil)
        }
        guard n > 0, let text = String(bytes: buf[0..<Int(n)], encoding: .utf8),
            text.hasPrefix("AETHER_CFG|")
        else {
            return nil
        }

        let parts = text.split(separator: "|", omittingEmptySubsequences: false)
        guard parts.count >= 2, !parts[1].isEmpty else { return nil }
        let ip = String(parts[1])
        let port = (parts.count >= 3 ? UInt16(parts[2]) : nil) ?? AetherProtocol.defaultAudioPort
        return (ip, port)
    }
}
