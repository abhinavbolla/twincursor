// Presses ⌃⌥⌘R the way a keyboard would, for bench/control-e2e.sh's revoke check: the three
// modifiers go down, then R, then everything comes back up. With the argument `m` it presses
// ⌃⌥⌘M instead, the sharer's mute hotkey (bench/audio-acceptance.sh).
// Posting events needs Accessibility permission for the terminal running it.
import CoreGraphics
import Foundation

let control: CGKeyCode = 59, option: CGKeyCode = 58, command: CGKeyCode = 55
let key: CGKeyCode = CommandLine.arguments.dropFirst().first == "m" ? 46 : 15
let source = CGEventSource(stateID: .hidSystemState)

func post(_ key: CGKeyCode, _ down: Bool, _ flags: CGEventFlags) {
    let event = CGEvent(keyboardEventSource: source, virtualKey: key, keyDown: down)!
    event.flags = flags
    event.post(tap: .cghidEventTap)
    usleep(20_000)
}

post(control, true, [.maskControl])
post(option, true, [.maskControl, .maskAlternate])
post(command, true, [.maskControl, .maskAlternate, .maskCommand])
post(key, true, [.maskControl, .maskAlternate, .maskCommand])
post(key, false, [.maskControl, .maskAlternate, .maskCommand])
post(command, false, [.maskControl, .maskAlternate])
post(option, false, [.maskControl])
post(control, false, [])
