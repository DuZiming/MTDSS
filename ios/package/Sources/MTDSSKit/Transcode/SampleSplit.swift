/// 样缓冲字节面(V4-1 T14;CoreMedia 数据面跨平台,macOS 宿主可测——
/// CMSampleBuffer 构造/检查为纯数据操作,零编解码,不越 Ruling E):
/// - **逐样切分**(``audioSamplePackets(from:)``):TrackOutput 原样包面
///   不保证 1 CMSampleBuffer = 1 AAC 帧——批量缓冲按 `NumSamples` /
///   `GetSampleSize`(逐样字节,累计 = 缓冲内偏移)/ `GetSampleTimeStamp`
///   (逐样 pts)逐样切分,每帧一枚 mux 入口(trun 样表逐帧正确面);
///   尺寸账不平/拷贝失败 = nil 如实弃(管线按缺席样跳过,勿以整缓冲
///   一枚假帧污样表)。
/// - **整缓冲取字节**(``wholeBufferData(from:)``):PCM 面专用——转编臂
///   喂 AudioConverter 按流消费,批量缓冲无帧界语义,整取即可。

import CoreMedia

/// 压缩样缓冲逐样切分(直通臂面):`[(ptsUs, data)]` 逐帧入口;账不平
/// (逐样尺寸和 ≠ 缓冲长)/样缺席/拷贝失败 = nil。
public func audioSamplePackets(from sb: CMSampleBuffer) -> [(ptsUs: Int64, data: [UInt8])]? {
    guard CMSampleBufferDataIsReady(sb), let bb = CMSampleBufferGetDataBuffer(sb) else {
        return nil
    }
    let count = CMSampleBufferGetNumSamples(sb)
    guard count > 0 else { return nil }
    // 逐样尺寸账(累计 = 缓冲内偏移;读出器原样包面恒连续顺序存储)
    var sizes = [Int](repeating: 0, count: count)
    var total = 0
    for i in 0..<count {
        let s = CMSampleBufferGetSampleSize(sb, at: CMItemCount(i))
        guard s > 0 else { return nil }
        sizes[i] = s
        total += s
    }
    // 尺寸账 vs 缓冲实长(不平 = 形异常,如实弃——勿以整缓冲一枚假帧污
    // trun 样表,评审 Important #1 面)
    guard total == CMBlockBufferGetDataLength(bb) else { return nil }
    var bytes = [UInt8](repeating: 0, count: total)
    let status = bytes.withUnsafeMutableBytes { raw in
        CMBlockBufferCopyDataBytes(
            bb, atOffset: 0, dataLength: total, destination: raw.baseAddress!)
    }
    guard status == kCMBlockBufferNoErr else { return nil }
    var out: [(ptsUs: Int64, data: [UInt8])] = []
    out.reserveCapacity(count)
    var offset = 0
    for i in 0..<count {
        // 逐样 pts(样级 timing;单 timing 条目缓冲回退整缓冲 pts)
        var info = CMSampleTimingInfo(
            duration: CMTime.invalid, presentationTimeStamp: CMTime.invalid,
            decodeTimeStamp: CMTime.invalid)
        var ptsUs: Int64
        if CMSampleBufferGetSampleTimingInfo(sb, at: CMItemCount(i), timingInfoOut: &info) == noErr,
           info.presentationTimeStamp.isNumeric {
            ptsUs = Int64((CMTimeGetSeconds(info.presentationTimeStamp) * 1_000_000).rounded())
        } else {
            ptsUs = Int64((CMTimeGetSeconds(
                CMSampleBufferGetPresentationTimeStamp(sb)) * 1_000_000).rounded())
        }
        out.append((ptsUs, Array(bytes[offset..<(offset + sizes[i])])))
        offset += sizes[i]
    }
    guard offset == bytes.count else { return nil }
    return out
}

/// 整缓冲字节(PCM 面:转编臂 AudioConverter 按流消费;样缺席 = nil)。
public func wholeBufferData(from sb: CMSampleBuffer) -> [UInt8]? {
    guard CMSampleBufferDataIsReady(sb), let bb = CMSampleBufferGetDataBuffer(sb) else {
        return nil
    }
    let length = CMBlockBufferGetDataLength(bb)
    guard length > 0 else { return nil }
    var bytes = [UInt8](repeating: 0, count: length)
    let status = bytes.withUnsafeMutableBytes { raw in
        CMBlockBufferCopyDataBytes(
            bb, atOffset: 0, dataLength: length, destination: raw.baseAddress!)
    }
    guard status == kCMBlockBufferNoErr else { return nil }
    return bytes
}
