import Darwin
import Foundation

/**
 * Wire-compatible AetherSora UDP PCM streaming protocol (48 kHz, 16-bit Stereo LE, 240 frames / 5 ms).
 */
enum AetherProtocol {
    static let magic: UInt16 = 0xA55A
    static let headerSize = 12
    static let sampleRate: UInt32 = 48_000
    static let channels: UInt8 = 2
    static let framesPerPacket = 240
    static let pcmBytesPerPacket = framesPerPacket * Int(channels) * 2
    static let maxPacketSize = headerSize + pcmBytesPerPacket

    static let flagAudio: UInt8 = 0x00
    static let flagKeepalive: UInt8 = 0x01
    static let flagHello: UInt8 = 0x02
    static let flagBye: UInt8 = 0x04

    static let ackMagic0: UInt8 = 0x5A
    static let ackMagic1: UInt8 = 0xA5

    static let defaultAudioPort: UInt16 = 19480
    static let discoveryPort: UInt16 = 19481
}

/**
 * Zero-allocation POSIX UDP sender speaking AetherSora HELLO/ACK/AUDIO/KEEPALIVE/BYE frames.
 */
final class AetherUdpSender {
    private var fd: Int32 = -1
    private var destAddr = sockaddr_in()
    private var packetBuffer = [UInt8](repeating: 0, count: AetherProtocol.maxPacketSize)
    private var seq: UInt32 = 1

    init?(host: String, port: UInt16) {
        var addr = sockaddr_in()
        addr.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_port = port.bigEndian

        let converted = host.withCString { cStr in
            inet_pton(AF_INET, cStr, &addr.sin_addr)
        }
        guard converted == 1 else {
            return nil
        }

        let sock = Darwin.socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP)
        guard sock >= 0 else {
            return nil
        }

        var tos: Int32 = 0xB8
        _ = Darwin.setsockopt(sock, IPPROTO_IP, IP_TOS, &tos, socklen_t(MemoryLayout<Int32>.size))

        var noSigPipe: Int32 = 1
        _ = Darwin.setsockopt(
            sock, SOL_SOCKET, SO_NOSIGPIPE, &noSigPipe, socklen_t(MemoryLayout<Int32>.size))

        var tv = timeval(tv_sec: 0, tv_usec: 350_000)
        _ = Darwin.setsockopt(sock, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))

        self.fd = sock
        self.destAddr = addr
    }

    deinit {
        close()
    }

    func sendHelloAndWaitAck() -> Bool {
        guard fd >= 0 else { return false }
        writeHeader(seq: 0, flags: AetherProtocol.flagHello)
        sendRawPacket(length: AetherProtocol.headerSize)

        var ackBuf = [UInt8](repeating: 0, count: 16)
        let received = ackBuf.withUnsafeMutableBytes { rawBuf -> ssize_t in
            guard let base = rawBuf.baseAddress else { return -1 }
            return Darwin.recvfrom(fd, base, 16, 0, nil, nil)
        }
        return received >= 4
            && ackBuf[0] == AetherProtocol.ackMagic0
            && ackBuf[1] == AetherProtocol.ackMagic1
    }

    func sendAudio(pcmBytes: UnsafeRawPointer, byteCount: Int) {
        guard fd >= 0, byteCount > 0, byteCount <= AetherProtocol.pcmBytesPerPacket else { return }
        writeHeader(seq: seq, flags: AetherProtocol.flagAudio)
        seq &+= 1

        packetBuffer.withUnsafeMutableBytes { rawBuf in
            guard let dst = rawBuf.baseAddress else { return }
            dst.advanced(by: AetherProtocol.headerSize).copyMemory(from: pcmBytes, byteCount: byteCount)
        }
        sendRawPacket(length: AetherProtocol.headerSize + byteCount)
    }

    func sendKeepalive() {
        guard fd >= 0 else { return }
        writeHeader(seq: seq, flags: AetherProtocol.flagKeepalive)
        sendRawPacket(length: AetherProtocol.headerSize)
    }

    func close() {
        guard fd >= 0 else { return }
        writeHeader(seq: 0, flags: AetherProtocol.flagBye)
        sendRawPacket(length: AetherProtocol.headerSize)
        sendRawPacket(length: AetherProtocol.headerSize)
        Darwin.close(fd)
        fd = -1
    }

    private func writeHeader(seq: UInt32, flags: UInt8) {
        packetBuffer[0] = UInt8(AetherProtocol.magic & 0xFF)
        packetBuffer[1] = UInt8((AetherProtocol.magic >> 8) & 0xFF)

        packetBuffer[2] = UInt8(seq & 0xFF)
        packetBuffer[3] = UInt8((seq >> 8) & 0xFF)
        packetBuffer[4] = UInt8((seq >> 16) & 0xFF)
        packetBuffer[5] = UInt8((seq >> 24) & 0xFF)

        let sr = AetherProtocol.sampleRate
        packetBuffer[6] = UInt8(sr & 0xFF)
        packetBuffer[7] = UInt8((sr >> 8) & 0xFF)
        packetBuffer[8] = UInt8((sr >> 16) & 0xFF)
        packetBuffer[9] = UInt8((sr >> 24) & 0xFF)

        packetBuffer[10] = AetherProtocol.channels
        packetBuffer[11] = flags
    }

    private func sendRawPacket(length: Int) {
        var addrCopy = destAddr
        withUnsafePointer(to: &addrCopy) { addrPtr in
            addrPtr.withMemoryRebound(to: sockaddr.self, capacity: 1) { saPtr in
                packetBuffer.withUnsafeBytes { bufPtr in
                    guard let base = bufPtr.baseAddress else { return }
                    _ = Darwin.sendto(
                        fd, base, length, 0, saPtr, socklen_t(MemoryLayout<sockaddr_in>.size))
                }
            }
        }
    }
}
