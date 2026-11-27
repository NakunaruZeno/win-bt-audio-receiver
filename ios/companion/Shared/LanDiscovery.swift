import Darwin
import Foundation

/**
 * Discovered AetherSora Windows PC receiver on the local Wi-Fi network.
 */
struct DiscoveredPC: Identifiable, Hashable {
    var id: String { "\(ip):\(audioPort)" }
    let hostname: String
    let ip: String
    let audioPort: UInt16
}

/**
 * Lightweight UDP LAN discovery scanner broadcasting `AETHERSORA_DISCOVER_V1` on UDP 19481
 * and collecting `AETHERSORA_PC_V1|<HOSTNAME>|<PORT>` replies from active Windows hosts.
 */
enum LanDiscovery {
    private static let discoverMagic = Array("AETHERSORA_DISCOVER_V1".utf8)
    private static let replyPrefix = "AETHERSORA_PC_V1|"

    static func scan(timeoutMs: Int = 1200) -> [DiscoveredPC] {
        let fd = Darwin.socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP)
        guard fd >= 0 else { return [] }
        defer { Darwin.close(fd) }

        var broadcastEnable: Int32 = 1
        _ = Darwin.setsockopt(
            fd, SOL_SOCKET, SO_BROADCAST, &broadcastEnable, socklen_t(MemoryLayout<Int32>.size))

        var tv = timeval(tv_sec: 0, tv_usec: 250_000)
        _ = Darwin.setsockopt(
            fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))

        for targetAddr in collectBroadcastTargets() {
            var addr = sockaddr_in()
            addr.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
            addr.sin_family = sa_family_t(AF_INET)
            addr.sin_port = AetherProtocol.discoveryPort.bigEndian
            addr.sin_addr = targetAddr

            withUnsafePointer(to: &addr) { ptr in
                ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { saPtr in
                    discoverMagic.withUnsafeBytes { raw in
                        guard let base = raw.baseAddress else { return }
                        _ = Darwin.sendto(
                            fd,
                            base,
                            discoverMagic.count,
                            0,
                            saPtr,
                            socklen_t(MemoryLayout<sockaddr_in>.size))
                    }
                }
            }
        }

        var foundByIp: [String: DiscoveredPC] = [:]
        var orderedIps: [String] = []
        var recvBuf = [UInt8](repeating: 0, count: 256)
        let deadlineNs = DispatchTime.now().uptimeNanoseconds + UInt64(max(200, timeoutMs)) * 1_000_000

        while DispatchTime.now().uptimeNanoseconds < deadlineNs {
            var srcAddr = sockaddr_in()
            var srcLen = socklen_t(MemoryLayout<sockaddr_in>.size)

            let n = recvBuf.withUnsafeMutableBytes { raw -> ssize_t in
                guard let base = raw.baseAddress else { return -1 }
                return withUnsafeMutablePointer(to: &srcAddr) { ptr in
                    ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { saPtr in
                        Darwin.recvfrom(fd, base, 256, 0, saPtr, &srcLen)
                    }
                }
            }

            if n <= 0 {
                if errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR {
                    continue
                }
                break
            }

            guard let msg = String(bytes: recvBuf[0..<Int(n)], encoding: .utf8)?
                .trimmingCharacters(in: .whitespacesAndNewlines),
                msg.hasPrefix(replyPrefix)
            else {
                continue
            }

            var ipChars = [CChar](repeating: 0, count: Int(INET_ADDRSTRLEN))
            var sinAddr = srcAddr.sin_addr
            guard inet_ntop(AF_INET, &sinAddr, &ipChars, socklen_t(INET_ADDRSTRLEN)) != nil else {
                continue
            }
            let ip = String(cString: ipChars)

            let parts = msg.split(separator: "|", omittingEmptySubsequences: false)
            let hostname = parts.count >= 2 && !parts[1].isEmpty ? String(parts[1]) : "Windows PC"
            let port = (parts.count >= 3 ? UInt16(parts[2]) : nil) ?? AetherProtocol.defaultAudioPort

            if foundByIp[ip] == nil {
                orderedIps.append(ip)
            }
            foundByIp[ip] = DiscoveredPC(hostname: hostname, ip: ip, audioPort: port)
        }

        return orderedIps.compactMap { foundByIp[$0] }
    }

    private static func collectBroadcastTargets() -> [in_addr] {
        let globalBroadcast = in_addr_t(0xFFFF_FFFF)
        var seen: Set<in_addr_t> = [globalBroadcast]
        var targets: [in_addr] = [in_addr(s_addr: globalBroadcast)]

        var ifaddrsPtr: UnsafeMutablePointer<ifaddrs>?
        guard Darwin.getifaddrs(&ifaddrsPtr) == 0, let first = ifaddrsPtr else {
            return targets
        }
        defer { Darwin.freeifaddrs(first) }

        var cursor: UnsafeMutablePointer<ifaddrs>? = first
        while let current = cursor {
            let flags = Int32(current.pointee.ifa_flags)
            let isUp = (flags & IFF_UP) != 0
            let isLoopback = (flags & IFF_LOOPBACK) != 0
            let hasBroadcast = (flags & IFF_BROADCAST) != 0

            if isUp, !isLoopback, hasBroadcast, let dst = current.pointee.ifa_dstaddr,
                dst.pointee.sa_family == sa_family_t(AF_INET)
            {
                let sin = dst.withMemoryRebound(to: sockaddr_in.self, capacity: 1) { $0.pointee }
                if !seen.contains(sin.sin_addr.s_addr) {
                    seen.insert(sin.sin_addr.s_addr)
                    targets.append(sin.sin_addr)
                }
            }
            cursor = current.pointee.ifa_next
        }
        return targets
    }
}
