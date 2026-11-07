//! `tc-spike replay`: checks that what reached the sharer is exactly what the guest sent.
//!
//! The viewer logs every message it sent (`view --sent-log`), and the sharer records every action
//! it injected (`share --inject record:PATH`). Replaying the sent messages through a fresh
//! [`Controller`] gives the actions a perfect network would produce. Keys, clicks, scrolls and text
//! must then match one for one and in order. Pointer moves travel on the lossy channel, so some
//! may be missing, but none may be added or reordered.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Context;
use control::{Action, Config, Controller, Effect, RecordInjector};
use protocol::{Button, InputEvent, Message, ScrollUnit};
use serde::de::DeserializeOwned;

/// With `revoked`, the sharer took control back partway: what was recorded must be the start of
/// what was sent, then only releases, leaving nothing held.
pub fn run(sent: &Path, actions: &Path, revoked: bool) -> anyhow::Result<()> {
    let messages: Vec<Message> =
        read_lines(sent).with_context(|| format!("reading {}", sent.display()))?;
    let recorded: Vec<Action> =
        read_lines(actions).with_context(|| format!("reading {}", actions.display()))?;
    let expected = simulate(&messages);
    if revoked {
        anyhow::ensure!(
            !messages.contains(&Message::ReleaseControl),
            "FAIL: the guest gave control back itself, so this run wasn't revoked"
        );
        return match compare_revoked(&expected, &recorded) {
            Ok(n) => {
                println!(
                    "PASS: the first {n} of {} keys, clicks, scrolls and characters arrived in order, then only releases, and nothing is left held",
                    split(&expected).1.len()
                );
                Ok(())
            }
            Err(e) => anyhow::bail!("FAIL: {e}"),
        };
    }
    let report = compare(&expected, &recorded);
    println!(
        "{} messages sent; {} actions expected, {} recorded",
        messages.len(),
        expected.len(),
        recorded.len()
    );
    match report {
        Ok(r) => {
            println!(
                "PASS: {} keys, clicks, scrolls and text identical and in order; {} of {} pointer moves arrived, none reordered",
                r.exact, r.moves_arrived, r.moves_expected
            );
            Ok(())
        }
        Err(e) => anyhow::bail!("FAIL: {e}"),
    }
}

/// Checks what a `tc-spike probe` window received against what the guest sent: the text typed,
/// the shortcuts pressed, the clicks (in order, within 2% of where they were aimed) and the
/// scroll directions.
pub fn run_probe(sent: &Path, probe: &Path) -> anyhow::Result<()> {
    let messages: Vec<Message> =
        read_lines(sent).with_context(|| format!("reading {}", sent.display()))?;
    let received: Vec<InputEvent> =
        read_lines(probe).with_context(|| format!("reading {}", probe.display()))?;
    let sent: Vec<InputEvent> = messages
        .into_iter()
        .filter_map(|m| match m {
            Message::Input { event, .. } => Some(event),
            _ => None,
        })
        .collect();
    match compare_probe(&sent, &received) {
        Ok(r) => {
            println!(
                "PASS: {} characters typed exactly, {} shortcut keys, {} clicks in place, {} scrolls the right way",
                r.chars, r.shortcuts, r.clicks, r.scrolls
            );
            Ok(())
        }
        Err(e) => anyhow::bail!("FAIL: {e}"),
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct ProbeReport {
    pub chars: usize,
    pub shortcuts: usize,
    pub clicks: usize,
    pub scrolls: usize,
}

#[derive(Default)]
struct Seen {
    text: String,
    shortcuts: Vec<String>,
    clicks: Vec<(Button, bool, f64, f64)>,
    scrolls: Vec<(ScrollUnit, f64, f64)>,
}

fn is_shortcut_modifier(code: &str) -> bool {
    matches!(
        code,
        "ControlLeft" | "ControlRight" | "MetaLeft" | "MetaRight"
    )
}

/// What a stream of events amounts to. `sent` says which side it is: the guest's keys carry no
/// text for Enter and Tab, where a window reports them as "\r" and "\t".
fn digest(events: &[InputEvent], sent: bool) -> Seen {
    let mut seen = Seen::default();
    let mut shortcut_held: Vec<String> = Vec::new();
    for event in events {
        match event {
            InputEvent::Key {
                code,
                pressed,
                text,
            } => {
                if is_shortcut_modifier(code) {
                    if *pressed {
                        shortcut_held.push(code.clone());
                    } else {
                        shortcut_held.retain(|c| c != code);
                    }
                    continue;
                }
                if !*pressed {
                    continue;
                }
                if shortcut_held.is_empty() {
                    let typed = match (text, code.as_str()) {
                        (Some(t), _) => Some(t.clone()),
                        (None, "Enter" | "NumpadEnter") if sent => Some("\r".into()),
                        (None, "Tab") if sent => Some("\t".into()),
                        _ => None,
                    };
                    seen.text.extend(typed);
                } else {
                    seen.shortcuts
                        .push(format!("{}+{code}", shortcut_held.join("+")));
                }
            }
            InputEvent::Button {
                button,
                pressed,
                x,
                y,
            } => seen.clicks.push((*button, *pressed, *x, *y)),
            InputEvent::Scroll { dx, dy, unit } => seen.scrolls.push((*unit, *dx, *dy)),
            InputEvent::Move { .. } | InputEvent::ReleaseAll => {}
        }
    }
    seen
}

pub fn compare_probe(sent: &[InputEvent], received: &[InputEvent]) -> Result<ProbeReport, String> {
    let want = digest(sent, true);
    let got = digest(received, false);
    if want.text != got.text {
        let at = want
            .text
            .chars()
            .zip(got.text.chars())
            .take_while(|(a, b)| a == b)
            .count();
        return Err(format!(
            "typed text differs at character {}: sent {:?}, received {:?}",
            at + 1,
            want.text.chars().skip(at).take(20).collect::<String>(),
            got.text.chars().skip(at).take(20).collect::<String>()
        ));
    }
    if want.shortcuts != got.shortcuts {
        return Err(format!(
            "shortcuts differ: sent {:?}, received {:?}",
            want.shortcuts, got.shortcuts
        ));
    }
    if want.clicks.len() != got.clicks.len() {
        return Err(format!(
            "{} button presses and releases sent, {} received",
            want.clicks.len(),
            got.clicks.len()
        ));
    }
    for (i, (w, g)) in want.clicks.iter().zip(&got.clicks).enumerate() {
        let near = (w.2 - g.2).abs() <= 0.02 && (w.3 - g.3).abs() <= 0.02;
        if (w.0, w.1) != (g.0, g.1) || !near {
            return Err(format!("click {}: sent {w:?}, received {g:?}", i + 1));
        }
    }
    // macOS may split or merge scroll events, so compare the total per unit and direction.
    for unit in [ScrollUnit::Pixel, ScrollUnit::Line] {
        let total = |v: &[(ScrollUnit, f64, f64)]| {
            v.iter()
                .filter(|s| s.0 == unit)
                .fold((0.0, 0.0), |(x, y), s| (x + s.1, y + s.2))
        };
        let (w, g) = (total(&want.scrolls), total(&got.scrolls));
        let same_way = |a: f64, b: f64| a == 0.0 || a.signum() == b.signum();
        if !same_way(w.0, g.0) || !same_way(w.1, g.1) {
            return Err(format!(
                "{unit:?} scrolling went the wrong way or nowhere: sent {w:?} in total, received {g:?}"
            ));
        }
    }
    Ok(ProbeReport {
        chars: want.text.chars().count(),
        shortcuts: want.shortcuts.len(),
        clicks: want.clicks.len(),
        scrolls: want.scrolls.len(),
    })
}

fn read_lines<T: DeserializeOwned>(path: &Path) -> anyhow::Result<Vec<T>> {
    std::fs::read_to_string(path)?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .enumerate()
        .map(|(i, l)| serde_json::from_str(l).with_context(|| format!("line {}", i + 1)))
        .collect()
}

/// The actions the sharer would inject if every message arrived at once, in order.
pub fn simulate(messages: &[Message]) -> Vec<Action> {
    let t0 = Instant::now();
    let mut controller = Controller::new(RecordInjector::new(Vec::new()), Config::default());
    let mut now = t0;
    for message in messages {
        if let Message::Input { sent_at_us, .. } | Message::Heartbeat { sent_at_us } = message {
            now = t0 + Duration::from_micros(*sent_at_us);
        }
        for effect in controller.handle("guest", message.clone(), now) {
            if matches!(effect, Effect::Ask(_)) {
                controller.grant(now);
            }
        }
    }
    // The sharer revokes when it stops, releasing anything still held.
    controller.revoke();
    let out =
        std::mem::replace(controller.injector_mut(), RecordInjector::new(Vec::new())).into_inner();
    String::from_utf8(out)
        .expect("utf-8")
        .lines()
        .map(|l| serde_json::from_str(l).expect("own output parses"))
        .collect()
}

#[derive(Debug, PartialEq)]
pub struct Report {
    pub exact: usize,
    pub moves_expected: usize,
    pub moves_arrived: usize,
}

pub fn compare(expected: &[Action], recorded: &[Action]) -> Result<Report, String> {
    let (exp_moves, exp_rest) = split(expected);
    let (rec_moves, rec_rest) = split(recorded);
    if let Some(i) =
        (0..exp_rest.len().max(rec_rest.len())).find(|&i| exp_rest.get(i) != rec_rest.get(i))
    {
        return Err(format!(
            "action {} of the keys, clicks, scrolls and text differs: expected {:?}, recorded {:?}",
            i + 1,
            exp_rest.get(i),
            rec_rest.get(i)
        ));
    }
    // Every recorded move must appear among the expected ones, in the same order.
    let mut rest = exp_moves.iter();
    for (i, m) in rec_moves.iter().enumerate() {
        if !rest.any(|e| e == m) {
            return Err(format!(
                "pointer move {} ({m:?}) was never sent, or arrived out of order",
                i + 1
            ));
        }
    }
    Ok(Report {
        exact: exp_rest.len(),
        moves_expected: exp_moves.len(),
        moves_arrived: rec_moves.len(),
    })
}

/// Checks a run the sharer revoked partway. Returns how many actions got through before it.
pub fn compare_revoked(expected: &[Action], recorded: &[Action]) -> Result<usize, String> {
    let (_, exp) = split(expected);
    let (_, rec) = split(recorded);
    // Messages the guest sent before it heard of the revoke are turned away, so the recording
    // may stop short of what was sent.
    let n = exp.iter().zip(&rec).take_while(|(e, r)| e == r).count();
    if let Some(a) = rec[n..].iter().find(|a| !is_release(a)) {
        return Err(format!(
            "after action {n}, {a:?} got through, but only releases may follow a revoke"
        ));
    }
    let held = still_held(recorded);
    if !held.is_empty() {
        return Err(format!("still held at the end: {held:?}"));
    }
    Ok(n)
}

fn is_release(a: &Action) -> bool {
    matches!(
        a,
        Action::Key { pressed: false, .. } | Action::Button { pressed: false, .. }
    )
}

/// Keys and buttons pressed and never released.
fn still_held(actions: &[Action]) -> Vec<String> {
    let mut held: Vec<String> = Vec::new();
    for a in actions {
        let (name, pressed) = match a {
            Action::Key { code, pressed } => (code.clone(), *pressed),
            Action::Button { button, pressed } => (format!("{button:?}"), *pressed),
            _ => continue,
        };
        if pressed {
            if !held.contains(&name) {
                held.push(name);
            }
        } else {
            held.retain(|h| *h != name);
        }
    }
    held
}

fn split(actions: &[Action]) -> (Vec<&Action>, Vec<&Action>) {
    actions.iter().partition(|a| matches!(a, Action::MoveTo(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use control::Point;

    fn mv(x: f64) -> Action {
        Action::MoveTo(Point { x, y: 0.5 })
    }

    fn text(t: &str) -> Action {
        Action::Text { text: t.into() }
    }

    #[test]
    fn missing_moves_pass_but_anything_else_fails() {
        let expected = [mv(0.1), mv(0.2), text("a"), mv(0.3), text("b")];
        let ok = compare(&expected, &[mv(0.1), text("a"), mv(0.3), text("b")]).unwrap();
        assert_eq!(
            ok,
            Report {
                exact: 2,
                moves_expected: 3,
                moves_arrived: 2
            }
        );
        assert!(
            compare(&expected, &[mv(0.1), text("a")]).is_err(),
            "lost text"
        );
        assert!(
            compare(&expected, &[text("b"), text("a")]).is_err(),
            "reordered text"
        );
        assert!(
            compare(&expected, &[mv(0.3), mv(0.1), text("a"), text("b")]).is_err(),
            "reordered moves"
        );
        assert!(
            compare(&expected, &[mv(0.9), text("a"), text("b")]).is_err(),
            "invented move"
        );
    }

    #[test]
    fn a_revoke_may_cut_the_run_short_but_must_release_everything() {
        let key = |code: &str, pressed| Action::Key {
            code: code.into(),
            pressed,
        };
        let expected = [
            key("ShiftLeft", true),
            text("a"),
            text("b"),
            key("ShiftLeft", false),
        ];
        assert_eq!(
            compare_revoked(
                &expected,
                &[key("ShiftLeft", true), text("a"), key("ShiftLeft", false)]
            ),
            Ok(2)
        );
        assert!(
            compare_revoked(&expected, &[key("ShiftLeft", true), text("a")]).is_err(),
            "shift left held"
        );
        assert!(
            compare_revoked(
                &expected,
                &[key("ShiftLeft", true), text("b"), key("ShiftLeft", false)]
            )
            .is_err(),
            "typing went on after the cut"
        );
        assert_eq!(
            compare_revoked(&expected, &expected),
            Ok(4),
            "the revoke landed after the last message"
        );
    }

    #[test]
    fn probe_comparison_checks_text_shortcuts_clicks_and_scrolls() {
        let key = |code: &str, pressed, text: Option<&str>| InputEvent::Key {
            code: code.into(),
            pressed,
            text: text.map(Into::into),
        };
        let click = |pressed, x| InputEvent::Button {
            button: Button::Left,
            pressed,
            x,
            y: 0.5,
        };
        let sent = vec![
            key("KeyH", true, Some("h")),
            key("KeyH", false, None),
            key("Unidentified", true, Some("ä")),
            key("Enter", true, None),
            key("MetaLeft", true, None),
            key("KeyA", true, Some("a")),
            key("KeyA", false, None),
            key("MetaLeft", false, None),
            click(true, 0.25),
            click(false, 0.25),
            InputEvent::Scroll {
                dx: 0.0,
                dy: 40.0,
                unit: ScrollUnit::Pixel,
            },
        ];
        // What a window reports: text typed with an unknown physical key, "\r" for Enter, and
        // the scroll split in two.
        let received = vec![
            key("KeyH", true, Some("h")),
            key("KeyA", true, Some("ä")),
            key("Enter", true, Some("\r")),
            key("MetaLeft", true, None),
            key("KeyA", true, None),
            key("MetaLeft", false, None),
            click(true, 0.251),
            click(false, 0.251),
            InputEvent::Scroll {
                dx: 0.0,
                dy: 30.0,
                unit: ScrollUnit::Pixel,
            },
            InputEvent::Scroll {
                dx: 0.0,
                dy: 10.0,
                unit: ScrollUnit::Pixel,
            },
        ];
        assert_eq!(
            compare_probe(&sent, &received),
            Ok(ProbeReport {
                chars: 3,
                shortcuts: 1,
                clicks: 2,
                scrolls: 1
            })
        );
        let mut wrong = received.clone();
        wrong[1] = key("KeyA", true, Some("a"));
        assert!(compare_probe(&sent, &wrong).is_err(), "wrong character");
        let mut wrong = received.clone();
        wrong[6] = click(true, 0.4);
        assert!(compare_probe(&sent, &wrong).is_err(), "click off target");
        let mut wrong = received.clone();
        wrong[8] = InputEvent::Scroll {
            dx: 0.0,
            dy: -30.0,
            unit: ScrollUnit::Pixel,
        };
        wrong[9] = wrong[8].clone();
        assert!(compare_probe(&sent, &wrong).is_err(), "scrolled up");
    }

    #[test]
    fn simulates_a_granted_session() {
        let input = |seq, event| Message::Input {
            seq,
            sent_at_us: seq * 1000,
            event,
        };
        let messages = [
            Message::Hello {
                name: "g".into(),
                platform: protocol::Platform::Macos,
            },
            Message::RequestControl,
            input(1, InputEvent::Move { x: 0.5, y: 0.5 }),
            input(
                2,
                InputEvent::Button {
                    button: Button::Left,
                    pressed: true,
                    x: 0.5,
                    y: 0.5,
                },
            ),
            Message::ReleaseControl,
        ];
        let actions = simulate(&messages);
        let press = |pressed| Action::Button {
            button: Button::Left,
            pressed,
        };
        // A click moves the pointer to its own position first. The held button is released when
        // the guest gives control back.
        assert_eq!(actions, [mv(0.5), mv(0.5), press(true), press(false)]);
    }
}
