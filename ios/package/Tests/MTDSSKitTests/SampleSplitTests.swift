/// 样缓冲逐样切分宿主单测(V4-1 T14 评审 Important #1 钉面;CoreMedia 数据
/// 面跨平台——CMSampleBuffer 合成构造为纯数据操作,零编解码):直通臂
/// TrackOutput 原样包面不保证 1 CMSampleBuffer = 1 AAC 帧——批量缓冲按
/// NumSamples/GetSampleSize/GetSampleTimeStamp 逐帧入 mux(trun 样表逐帧
/// 正确面);账不平(逐样尺寸和 ≠ 缓冲实长)= nil 如实弃。
import CoreMedia
import Foundation
@testable import MTDSSKit
import XCTest

final class SampleSplitTests: XCTestCase {

    /// 合成多/单样 CMSampleBuffer(AAC ASBD 面;sizes/pts 直配;data =
    /// 连续缓冲字节)。
    private func makeSampleBuffer(
        sizes: [Int], ptsUs: [Int64], data: [UInt8],
    ) throws -> CMSampleBuffer {
        var sampleSizes = sizes
        var asbd = AudioStreamBasicDescription(
            mSampleRate: 48_000, mFormatID: kAudioFormatMPEG4AAC, mFormatFlags: 0,
            mBytesPerPacket: 0, mFramesPerPacket: 1024, mBytesPerFrame: 0,
            mChannelsPerFrame: 2, mBitsPerChannel: 0, mReserved: 0)
        var format: CMAudioFormatDescription?
        XCTAssertEqual(
            CMAudioFormatDescriptionCreate(
                allocator: nil, asbd: &asbd, layoutSize: 0, layout: nil,
                magicCookieSize: 0, magicCookie: nil, extensions: nil,
                formatDescriptionOut: &format),
            noErr)
        var block: CMBlockBuffer?
        XCTAssertEqual(
            CMBlockBufferCreateWithMemoryBlock(
                allocator: nil, memoryBlock: nil, blockLength: data.count,
                blockAllocator: nil, customBlockSource: nil, offsetToData: 0,
                dataLength: data.count, flags: 0, blockBufferOut: &block),
            kCMBlockBufferNoErr)
        let replaceStatus = data.withUnsafeBytes { raw in
            CMBlockBufferReplaceDataBytes(
                with: raw.baseAddress!, blockBuffer: block!, offsetIntoDestination: 0,
                dataLength: data.count)
        }
        XCTAssertEqual(replaceStatus, kCMBlockBufferNoErr)
        var timing = ptsUs.map {
            CMSampleTimingInfo(
                duration: CMTime.invalid,
                presentationTimeStamp: CMTime(value: CMTimeValue($0), timescale: 1_000_000),
                decodeTimeStamp: CMTime.invalid)
        }
        var sampleBuffer: CMSampleBuffer?
        XCTAssertEqual(
            CMSampleBufferCreateReady(
                allocator: nil, dataBuffer: block!, formatDescription: format,
                sampleCount: sampleSizes.count, sampleTimingEntryCount: timing.count,
                sampleTimingArray: &timing, sampleSizeEntryCount: sampleSizes.count,
                sampleSizeArray: &sampleSizes, sampleBufferOut: &sampleBuffer),
            noErr)
        return try XCTUnwrap(sampleBuffer)
    }

    /// 批量缓冲(3 帧)逐样切分:多 mux 入口 + 逐样尺寸切片正确 + 逐样
    /// pts 正确(评审 Important #1 主钉面)。
    func testMultiSampleBufferSplitsPerSample() throws {
        let data: [UInt8] = (0..<20).map { UInt8($0 + 1) } // 1..20
        let sb = try makeSampleBuffer(
            sizes: [4, 6, 10],
            ptsUs: [1_000_000, 1_021_000, 1_042_000],
            data: data)

        let packets = try XCTUnwrap(audioSamplePackets(from: sb))
        XCTAssertEqual(packets.count, 3, "批量缓冲 = 3 mux 入口(逐帧面)")
        XCTAssertEqual(packets[0].ptsUs, 1_000_000)
        XCTAssertEqual(packets[0].data, Array(data[0..<4]))
        XCTAssertEqual(packets[1].ptsUs, 1_021_000)
        XCTAssertEqual(packets[1].data, Array(data[4..<10]))
        XCTAssertEqual(packets[2].ptsUs, 1_042_000)
        XCTAssertEqual(packets[2].data, Array(data[10..<20]))
    }

    /// 单样缓冲 = 一枚入口整包(旧形等价面;pts 自样级时间戳)。
    func testSingleSampleBufferOnePacket() throws {
        let data: [UInt8] = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE]
        let sb = try makeSampleBuffer(sizes: [5], ptsUs: [500_000], data: data)

        let packets = try XCTUnwrap(audioSamplePackets(from: sb))
        XCTAssertEqual(packets.count, 1)
        XCTAssertEqual(packets[0].ptsUs, 500_000)
        XCTAssertEqual(packets[0].data, data)
    }

    /// 尺寸账不平(逐样尺寸和 ≠ 缓冲实长)= nil 如实弃(勿以整缓冲一枚
    /// 假帧污 trun 样表)。
    func testSizeAccountMismatchYieldsNil() throws {
        let data: [UInt8] = (0..<20).map { UInt8($0 + 1) }
        let sb = try makeSampleBuffer(
            sizes: [4, 6], // 和 = 10 ≠ 缓冲 20
            ptsUs: [1_000_000, 1_021_000],
            data: data)

        XCTAssertNil(audioSamplePackets(from: sb), "账不平如实弃")
    }
}
