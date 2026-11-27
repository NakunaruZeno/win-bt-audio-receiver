import AVFoundation
import CoreMedia
import Foundation

/**
 * Converts arbitrary ReplayKit `.audioApp` `CMSampleBuffer` audio formats (Float32/Int16,
 * mono/stereo, big/little-endian, 44.1/48 kHz) into exact 960-byte (240 stereo frames @ 48 kHz
 * 16-bit LE) packets ready for AetherSora UDP framing, computing fast normalized RMS telemetry.
 */
final class AudioPcmConverter {
    private struct SourceKey: Equatable {
        let sampleRate: Double
        let channels: UInt32
        let formatFlags: AudioFormatFlags
        let bitsPerChannel: UInt32
    }

    private let targetFormat: AVAudioFormat = {
        AVAudioFormat(
            commonFormat: .pcmFormatInt16,
            sampleRate: Double(AetherProtocol.sampleRate),
            channels: AVAudioChannelCount(AetherProtocol.channels),
            interleaved: true)!
    }()

    private var lastSourceKey: SourceKey?
    private var normalizedSourceFormat: AVAudioFormat?
    private var converter: AVAudioConverter?
    private var needsBigEndianSwap = false
    private var isInterleavedSource = true
    private var sourceCommonFormat: AVAudioCommonFormat = .pcmFormatFloat32

    private var fifo = [UInt8]()
    private var fifoReadOffset = 0

    func reset() {
        converter = nil
        normalizedSourceFormat = nil
        lastSourceKey = nil
        fifo.removeAll(keepingCapacity: true)
        fifoReadOffset = 0
    }

    func process(
        sampleBuffer: CMSampleBuffer,
        onPacket: (UnsafeRawPointer, Int, Float) -> Void
    ) {
        guard let fmtDesc = CMSampleBufferGetFormatDescription(sampleBuffer),
            let asbdPtr = CMAudioFormatDescriptionGetStreamBasicDescription(fmtDesc)
        else {
            return
        }

        let asbd = asbdPtr.pointee
        guard asbd.mFormatID == kAudioFormatLinearPCM,
            asbd.mChannelsPerFrame >= 1,
            asbd.mSampleRate > 0
        else {
            return
        }

        let key = SourceKey(
            sampleRate: asbd.mSampleRate,
            channels: asbd.mChannelsPerFrame,
            formatFlags: asbd.mFormatFlags,
            bitsPerChannel: asbd.mBitsPerChannel)

        if key != lastSourceKey {
            reconfigurePipeline(asbd: asbd, key: key)
        }

        guard let sourceFormat = normalizedSourceFormat,
            let activeConverter = converter
        else {
            return
        }

        let numSamples = CMSampleBufferGetNumSamples(sampleBuffer)
        guard numSamples > 0 else { return }

        var bufferListSizeNeeded = 0
        CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
            sampleBuffer,
            bufferListSizeNeededOut: &bufferListSizeNeeded,
            bufferListOut: nil,
            bufferListSize: 0,
            blockBufferAllocator: nil,
            blockBufferMemoryAllocator: nil,
            flags: 0,
            blockBufferOut: nil)

        guard bufferListSizeNeeded >= MemoryLayout<AudioBufferList>.size else { return }

        let ablRaw = UnsafeMutableRawPointer.allocate(
            byteCount: bufferListSizeNeeded,
            alignment: MemoryLayout<AudioBufferList>.alignment)
        defer { ablRaw.deallocate() }

        let ablPtr = ablRaw.assumingMemoryBound(to: AudioBufferList.self)
        var retainedBlock: CMBlockBuffer?
        let status = CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
            sampleBuffer,
            bufferListSizeNeededOut: nil,
            bufferListOut: ablPtr,
            bufferListSize: bufferListSizeNeeded,
            blockBufferAllocator: kCFAllocatorDefault,
            blockBufferMemoryAllocator: kCFAllocatorDefault,
            flags: UInt32(kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment),
            blockBufferOut: &retainedBlock)

        guard status == noErr else { return }
        defer { _ = retainedBlock }

        let frameCount = AVAudioFrameCount(numSamples)
        guard let srcBuffer = AVAudioPCMBuffer(pcmFormat: sourceFormat, frameCapacity: frameCount) else {
            return
        }
        srcBuffer.frameLength = frameCount

        let srcBuffers = UnsafeMutableAudioBufferListPointer(ablPtr)
        let dstBuffers = UnsafeMutableAudioBufferListPointer(srcBuffer.mutableAudioBufferList)
        let copyCount = min(srcBuffers.count, dstBuffers.count)

        for idx in 0..<copyCount {
            let srcBuf = srcBuffers[idx]
            let dstBuf = dstBuffers[idx]
            guard let srcData = srcBuf.mData, let dstData = dstBuf.mData else { continue }
            let byteCount = min(Int(srcBuf.mDataByteSize), Int(dstBuf.mDataByteSize))
            guard byteCount > 0 else { continue }

            if needsBigEndianSwap && sourceCommonFormat == .pcmFormatInt16 {
                let sampleTotal = byteCount / 2
                let src16 = srcData.assumingMemoryBound(to: UInt16.self)
                let dst16 = dstData.assumingMemoryBound(to: UInt16.self)
                for s in 0..<sampleTotal {
                    dst16[s] = UInt16(bigEndian: src16[s])
                }
            } else {
                dstData.copyMemory(from: srcData, byteCount: byteCount)
            }
        }

        let outRatio = targetFormat.sampleRate / sourceFormat.sampleRate
        let estimatedOutFrames = AVAudioFrameCount(ceil(Double(frameCount) * outRatio)) + 32
        guard let outBuffer = AVAudioPCMBuffer(pcmFormat: targetFormat, frameCapacity: estimatedOutFrames)
        else {
            return
        }

        var consumed = false
        var convError: NSError?
        let convStatus = activeConverter.convert(to: outBuffer, error: &convError) { _, outStatus in
            if consumed {
                outStatus.pointee = .noDataNow
                return nil
            }
            consumed = true
            outStatus.pointee = .haveData
            return srcBuffer
        }

        guard convStatus != .error, convError == nil, outBuffer.frameLength > 0 else {
            return
        }

        let outAbl = UnsafeMutableAudioBufferListPointer(outBuffer.mutableAudioBufferList)
        guard let firstOut = outAbl.first, let outPtr = firstOut.mData else { return }
        let producedBytes = Int(outBuffer.frameLength) * Int(AetherProtocol.channels) * 2
        let byteSlice = UnsafeBufferPointer(
            start: outPtr.assumingMemoryBound(to: UInt8.self),
            count: producedBytes)

        fifo.append(contentsOf: byteSlice)
        drainPackets(onPacket: onPacket)
    }

    private func reconfigurePipeline(asbd: AudioStreamBasicDescription, key: SourceKey) {
        lastSourceKey = key
        let isFloat = (asbd.mFormatFlags & kAudioFormatFlagIsFloat) != 0
        let commonFmt: AVAudioCommonFormat
        if isFloat {
            commonFmt = .pcmFormatFloat32
        } else if asbd.mBitsPerChannel == 32 {
            commonFmt = .pcmFormatInt32
        } else {
            commonFmt = .pcmFormatInt16
        }

        let nonInterleaved = (asbd.mFormatFlags & kAudioFormatFlagIsNonInterleaved) != 0
        needsBigEndianSwap = (asbd.mFormatFlags & kAudioFormatFlagIsBigEndian) != 0
        isInterleavedSource = !nonInterleaved
        sourceCommonFormat = commonFmt

        let channelCount = AVAudioChannelCount(min(max(asbd.mChannelsPerFrame, 1), 2))
        guard let normFmt = AVAudioFormat(
            commonFormat: commonFmt,
            sampleRate: asbd.mSampleRate,
            channels: channelCount,
            interleaved: isInterleavedSource),
            let newConverter = AVAudioConverter(from: normFmt, to: targetFormat)
        else {
            normalizedSourceFormat = nil
            converter = nil
            return
        }

        if channelCount == 1 {
            newConverter.channelMap = [NSNumber(value: 0), NSNumber(value: 0)]
        }

        normalizedSourceFormat = normFmt
        converter = newConverter
    }

    private func drainPackets(onPacket: (UnsafeRawPointer, Int, Float) -> Void) {
        let packetSize = AetherProtocol.pcmBytesPerPacket
        while fifo.count - fifoReadOffset >= packetSize {
            let offset = fifoReadOffset
            let rms = fifo.withUnsafeBytes { raw -> Float in
                guard let base = raw.baseAddress else { return 0 }
                let packetPtr = base.advanced(by: offset)
                let level = measureRmsNormalized(pcmPtr: packetPtr, byteCount: packetSize)
                onPacket(packetPtr, packetSize, level)
                return level
            }
            _ = rms
            fifoReadOffset += packetSize
        }

        if fifoReadOffset > 0 {
            if fifoReadOffset >= fifo.count {
                fifo.removeAll(keepingCapacity: true)
                fifoReadOffset = 0
            } else if fifoReadOffset >= packetSize * 8 {
                fifo.removeFirst(fifoReadOffset)
                fifoReadOffset = 0
            }
        }
    }

    private func measureRmsNormalized(pcmPtr: UnsafeRawPointer, byteCount: Int) -> Float {
        let sampleCount = byteCount / 2
        guard sampleCount > 0 else { return 0 }
        let samples = pcmPtr.assumingMemoryBound(to: Int16.self)
        var sumSq: Int64 = 0
        var allZero = true

        for i in 0..<sampleCount {
            let val = Int16(littleEndian: samples[i])
            if val != 0 {
                allZero = false
                let v = Int64(val)
                sumSq += v * v
            }
        }
        if allZero {
            return 0
        }
        let rms = sqrt(Double(sumSq) / Double(sampleCount))
        return Float(min(1.0, rms / 12_000.0))
    }
}
