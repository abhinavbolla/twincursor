// Records the default microphone, raw (no echo cancellation, noise suppression or gain control),
// and writes it to stdout for `tc-spike listen`: 48 kHz mono 16-bit, in chunks of
//   u64 LE  wall-clock time of the chunk's first sample, in µs since the Unix epoch
//   u32 LE  sample count n
//   n × i16 LE samples
// Times come from the tap's capture timestamps, not from when a chunk reached us. The mic may run
// at another rate (44.1 kHz on a MacBook), and the resampler's output doesn't line up with its
// input buffers, so each output chunk is timed through the input sample clock.
// Needs microphone access for the terminal running it. Stops on SIGINT/SIGTERM or a closed pipe.
import AVFoundation
import Foundation

setvbuf(stdout, nil, _IONBF, 0)
signal(SIGPIPE, SIG_IGN)

switch AVCaptureDevice.authorizationStatus(for: .audio) {
case .denied, .restricted:
    FileHandle.standardError.write(
        "record-mic: microphone access is off for this terminal (System Settings > Privacy & Security > Microphone)\n"
            .data(using: .utf8)!)
    exit(2)
default: break
}

let engine = AVAudioEngine()
let input = engine.inputNode
let inFormat = input.outputFormat(forBus: 0)
let outFormat = AVAudioFormat(
    commonFormat: .pcmFormatInt16, sampleRate: 48_000, channels: 1, interleaved: true)!
guard let converter = AVAudioConverter(from: inFormat, to: outFormat) else {
    FileHandle.standardError.write("record-mic: can't convert from \(inFormat)\n".data(using: .utf8)!)
    exit(1)
}
FileHandle.standardError.write("record-mic: \(inFormat.sampleRate) Hz, \(inFormat.channelCount) ch\n".data(using: .utf8)!)

var timebase = mach_timebase_info_data_t()
mach_timebase_info(&timebase)
func hostToWallMicros(_ host: UInt64) -> UInt64 {
    let now = mach_absolute_time()
    let wallNow = UInt64(Date().timeIntervalSince1970 * 1_000_000)
    let agoTicks = now > host ? now - host : 0
    let agoMicros = agoTicks * UInt64(timebase.numer) / UInt64(timebase.denom) / 1000
    return wallNow - agoMicros
}

let out = FileHandle.standardOutput
var failed = false
// Input samples so far, and where recent input buffers started: (input sample index, wall µs).
var inputTotal: Int64 = 0
var anchors: [(Int64, UInt64)] = []
var outputTotal: Int64 = 0

/// Wall-clock time of output sample `index`, from the input buffer it came from.
func wallMicros(ofOutput index: Int64) -> UInt64? {
    let inputIndex = Double(index) * inFormat.sampleRate / 48_000
    guard let anchor = anchors.last(where: { Double($0.0) <= inputIndex }) else { return nil }
    return anchor.1 + UInt64((inputIndex - Double(anchor.0)) / inFormat.sampleRate * 1_000_000)
}

input.installTap(onBus: 0, bufferSize: 480, format: inFormat) { buffer, when in
    if failed { return }
    guard when.isHostTimeValid else { return }
    anchors.append((inputTotal, hostToWallMicros(when.hostTime)))
    if anchors.count > 64 { anchors.removeFirst() }
    inputTotal += Int64(buffer.frameLength)
    let capacity = AVAudioFrameCount(Double(buffer.frameLength) * 48_000 / inFormat.sampleRate) + 16
    guard let converted = AVAudioPCMBuffer(pcmFormat: outFormat, frameCapacity: capacity) else { return }
    var fed = false
    var error: NSError?
    converter.convert(to: converted, error: &error) { _, status in
        if fed {
            status.pointee = .noDataNow
            return nil
        }
        fed = true
        status.pointee = .haveData
        return buffer
    }
    let n = Int(converted.frameLength)
    guard error == nil, n > 0 else { return }
    let start = outputTotal
    outputTotal += Int64(n)
    guard let wall = wallMicros(ofOutput: start) else { return }
    var header = Data(count: 12)
    header.withUnsafeMutableBytes { raw in
        raw.storeBytes(of: wall.littleEndian, toByteOffset: 0, as: UInt64.self)
        raw.storeBytes(of: UInt32(n).littleEndian, toByteOffset: 8, as: UInt32.self)
    }
    let samples = Data(bytes: converted.int16ChannelData![0], count: n * 2)
    do {
        try out.write(contentsOf: header + samples)
    } catch {
        failed = true
        exit(0)
    }
}

do {
    try engine.start()
} catch {
    FileHandle.standardError.write("record-mic: couldn't start: \(error)\n".data(using: .utf8)!)
    exit(1)
}
for sig in [SIGINT, SIGTERM] {
    signal(sig) { _ in exit(0) }
}
RunLoop.main.run()
