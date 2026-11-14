// Prints the latency the default mic and speakers report about themselves: the part of
// mouth-to-ear that belongs to the hardware and its driver, before any app's buffers.
// Reads properties only: nothing is played or recorded, so it needs no permission.
//   swiftc -O -o /tmp/device-latency bench/audio/device-latency.swift && /tmp/device-latency
import CoreAudio
import Foundation

func address(_ selector: AudioObjectPropertySelector, _ scope: AudioObjectPropertyScope)
    -> AudioObjectPropertyAddress
{
    AudioObjectPropertyAddress(
        mSelector: selector, mScope: scope, mElement: kAudioObjectPropertyElementMain)
}

func uint32(_ id: AudioObjectID, _ selector: AudioObjectPropertySelector, _ scope: AudioObjectPropertyScope) -> UInt32 {
    var addr = address(selector, scope)
    var value: UInt32 = 0
    var size = UInt32(MemoryLayout<UInt32>.size)
    return AudioObjectGetPropertyData(id, &addr, 0, nil, &size, &value) == noErr ? value : 0
}

func rate(_ id: AudioObjectID) -> Float64 {
    var addr = address(kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal)
    var value: Float64 = 0
    var size = UInt32(MemoryLayout<Float64>.size)
    return AudioObjectGetPropertyData(id, &addr, 0, nil, &size, &value) == noErr ? value : 0
}

func name(_ id: AudioObjectID) -> String {
    var addr = address(kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal)
    var value: Unmanaged<CFString>?
    var size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
    guard AudioObjectGetPropertyData(id, &addr, 0, nil, &size, &value) == noErr, let name = value
    else { return "?" }
    return name.takeRetainedValue() as String
}

func firstStream(_ id: AudioObjectID, _ scope: AudioObjectPropertyScope) -> AudioStreamID? {
    var addr = address(kAudioDevicePropertyStreams, scope)
    var size: UInt32 = 0
    guard AudioObjectGetPropertyDataSize(id, &addr, 0, nil, &size) == noErr, size > 0 else { return nil }
    var streams = [AudioStreamID](repeating: 0, count: Int(size) / MemoryLayout<AudioStreamID>.size)
    guard AudioObjectGetPropertyData(id, &addr, 0, nil, &size, &streams) == noErr else { return nil }
    return streams.first
}

let system = AudioObjectID(kAudioObjectSystemObject)
var total = 0.0
for (label, which, scope) in [
    ("mic", kAudioHardwarePropertyDefaultInputDevice, kAudioObjectPropertyScopeInput),
    ("speakers", kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyScopeOutput),
] {
    let device = uint32(system, which, kAudioObjectPropertyScopeGlobal)
    let hz = rate(device)
    guard hz > 0 else {
        print("\(label): no device")
        continue
    }
    let ms = { (frames: UInt32) in Double(frames) * 1000 / hz }
    let deviceLatency = uint32(device, kAudioDevicePropertyLatency, scope)
    let safety = uint32(device, kAudioDevicePropertySafetyOffset, scope)
    let stream = firstStream(device, scope).map {
        uint32($0, kAudioStreamPropertyLatency, kAudioObjectPropertyScopeGlobal)
    } ?? 0
    let sum = ms(deviceLatency) + ms(stream) + ms(safety)
    total += sum
    print(
        String(
            format: "%@ (%@, %.0f Hz): %.1f ms = device %.1f + stream %.1f + safety offset %.1f",
            label, name(device), hz, sum, ms(deviceLatency), ms(stream), ms(safety)))
}
print(String(format: "mic and speakers together: %.1f ms, plus each app's own I/O buffers", total))
