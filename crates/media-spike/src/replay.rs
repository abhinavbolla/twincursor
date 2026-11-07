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
use protocol::Message;
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
    use protocol::{Button, InputEvent};

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
