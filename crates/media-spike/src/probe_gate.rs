//! `tc-spike probe-gate`: waits on what a `tc-spike probe` window has recorded, for
//! `bench/control-inject.sh`, so that real input goes out one checked step at a time.
//!
//! The control script has the shape this checks ([`plan`]): only pointer moves before its first
//! `gate` (the last of them is the canary), no keys before its second, and nothing aimed outside
//! the probe window. The bench opens each gate only once the probe shows that what came before
//! it arrived where it was aimed: first that the window is ready, then the canary move, then
//! every click. Any `lost_focus`, `not_ready` or `closed` record, keys that nobody sent yet, or
//! the probe process going away fail the wait at once.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::{Args, ValueEnum};
use engine::Step;
use protocol::{Button, InputEvent};

use crate::probe::{Frame, ProbeLine};

#[derive(Args)]
pub struct ProbeGateArgs {
    /// What to wait for.
    what: Want,
    /// The probe's output file.
    #[arg(long)]
    probe: PathBuf,
    /// The control script the guest plays.
    #[arg(long)]
    script: PathBuf,
    /// The probe's process: fail as soon as it is gone.
    #[arg(long)]
    pid: Option<u32>,
    /// Give up after this many seconds (0: never, for `watch`).
    #[arg(long, default_value_t = 5.0)]
    timeout: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Want {
    /// The window is ready, and everything the script aims at is on it.
    Ready,
    /// The canary move arrived where it was aimed.
    Canary,
    /// Every click before the second gate arrived, in order and in place.
    Buttons,
    /// Nothing: fail only when the window loses focus or closes.
    Watch,
    /// Print the canary point and the clicks before the second gate, and exit.
    Plan,
}

/// How near a received position must be to the one sent (a fraction of the display).
const NEAR: f64 = 0.02;
/// How far inside the window's edges every aimed point must be.
const MARGIN: f64 = 0.01;

/// What a script will send, gate by gate.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// The last move before the first gate.
    pub canary: (f64, f64),
    /// Buttons between the first gate and the second (or the end), as the probe will see them.
    pub buttons: Vec<(Button, bool, f64, f64)>,
    /// Every position the script aims at.
    pub points: Vec<(f64, f64)>,
    /// Whether the script has a second gate, after which keys may come.
    pub keys_gate: bool,
}

/// Checks the script's shape and says what it will send. See the module docs.
pub fn plan(steps: &[Step]) -> Result<Plan, String> {
    let mut gates = 0;
    let mut canary = None;
    let mut buttons = Vec::new();
    let mut points = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        let n = i + 1;
        let event = match step {
            Step::Gate { .. } => {
                gates += 1;
                if gates == 1 && canary.is_none() {
                    return Err(format!(
                        "step {n}: the first gate must come after a pointer move (the canary)"
                    ));
                }
                if gates > 2 {
                    return Err(format!("step {n}: at most two gates"));
                }
                continue;
            }
            Step::Input { event } => Some(event),
            Step::Type { .. } => None,
            Step::Request
            | Step::AwaitControl
            | Step::AwaitEnd
            | Step::Release
            | Step::Wait { .. } => continue,
        };
        let keys =
            matches!(step, Step::Type { .. }) || matches!(event, Some(InputEvent::Key { .. }));
        match gates {
            0 if !matches!(event, Some(InputEvent::Move { .. })) => {
                return Err(format!(
                    "step {n}: only pointer moves may come before the first gate"
                ))
            }
            0 | 1 if keys => {
                return Err(format!(
                    "step {n}: keys and text may only come after the second gate"
                ))
            }
            _ => {}
        }
        match event {
            Some(InputEvent::Move { x, y }) => {
                points.push((*x, *y));
                if gates == 0 {
                    canary = Some((*x, *y));
                }
            }
            Some(InputEvent::Button {
                button,
                pressed,
                x,
                y,
            }) => {
                points.push((*x, *y));
                if gates == 1 {
                    buttons.push((*button, *pressed, *x, *y));
                }
            }
            _ => {}
        }
    }
    let canary = canary.ok_or("the script has no gate: give it one after a first pointer move")?;
    if gates == 0 {
        return Err("the script has no gate: give it one after a first pointer move".into());
    }
    Ok(Plan {
        canary,
        buttons,
        points,
        keys_gate: gates == 2,
    })
}

/// Where a wait stands.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Waiting,
    Met(String),
    Failed(String),
}

fn near(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).abs() <= NEAR && (a.1 - b.1).abs() <= NEAR
}

/// Where `want` stands, given what the probe has written so far.
pub fn judge(lines: &[ProbeLine], want: Want, plan: &Plan) -> Verdict {
    let mut ready = None;
    let mut events = Vec::new();
    for line in lines {
        match line {
            ProbeLine::NotReady { not_ready } => return Verdict::Failed(not_ready.clone()),
            ProbeLine::LostFocus { lost_focus } => {
                return Verdict::Failed(format!(
                    "the test window lost focus: {}",
                    lost_focus.problems()
                ))
            }
            ProbeLine::Closed { .. } => {
                return Verdict::Failed("the test window closed".into());
            }
            ProbeLine::Ready { frame, .. } => {
                if ready.is_none() {
                    ready = Some(*frame);
                }
            }
            ProbeLine::Event(event) if ready.is_some() => events.push(event),
            ProbeLine::Event(_) => {
                return Verdict::Failed("the probe recorded input before it was ready".into())
            }
        }
    }
    let Some(frame) = ready else {
        return Verdict::Waiting;
    };
    if let Some(at) = outside(&frame, &plan.points) {
        return Verdict::Failed(format!(
            "the script aims at {at:?}, which isn't on the test window ({frame:?})"
        ));
    }
    // Keys before the bench let any through came from someone at the keyboard, or from
    // somewhere the test doesn't know about.
    if want != Want::Watch && events.iter().any(|e| matches!(e, InputEvent::Key { .. })) {
        return Verdict::Failed(
            "the test window got key presses before any were sent: hands off the keyboard".into(),
        );
    }
    match want {
        Want::Ready => Verdict::Met(format!("the test window is ready ({frame:?})")),
        Want::Canary => {
            match events
                .iter()
                .find(|e| matches!(e, InputEvent::Move { x, y } if near((*x, *y), plan.canary)))
            {
                Some(_) => Verdict::Met(format!("the canary move arrived at {:?}", plan.canary)),
                None => Verdict::Waiting,
            }
        }
        Want::Buttons => {
            let got: Vec<_> = events
                .iter()
                .filter_map(|e| match e {
                    InputEvent::Button {
                        button,
                        pressed,
                        x,
                        y,
                    } => Some((*button, *pressed, *x, *y)),
                    _ => None,
                })
                .collect();
            for (i, (g, w)) in got.iter().zip(&plan.buttons).enumerate() {
                if (g.0, g.1) != (w.0, w.1) || !near((g.2, g.3), (w.2, w.3)) {
                    return Verdict::Failed(format!(
                        "button event {} arrived as {g:?}, sent as {w:?}",
                        i + 1
                    ));
                }
            }
            if got.len() > plan.buttons.len() {
                return Verdict::Failed(format!(
                    "{} button events arrived, {} were sent",
                    got.len(),
                    plan.buttons.len()
                ));
            }
            if got.len() == plan.buttons.len() {
                Verdict::Met(format!("all {} button events arrived in place", got.len()))
            } else {
                Verdict::Waiting
            }
        }
        Want::Watch | Want::Plan => Verdict::Waiting,
    }
}

fn outside(frame: &Frame, points: &[(f64, f64)]) -> Option<(f64, f64)> {
    points.iter().copied().find(|p| !frame.contains(*p, MARGIN))
}

/// Reads the probe's file. A line still being written is left for the next read.
fn read(path: &std::path::Path) -> Vec<ProbeLine> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map_while(|l| serde_json::from_str(l).ok())
        .collect()
}

fn alive(pid: u32) -> bool {
    std::process::Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Exit codes: 0 met, 1 failed, 2 timed out.
pub fn run(args: ProbeGateArgs) -> anyhow::Result<()> {
    let steps = engine::read_script(&args.script)
        .with_context(|| format!("reading {}", args.script.display()))?;
    let plan = match plan(&steps) {
        Ok(plan) => plan,
        Err(e) => {
            println!("the control script can't be checked step by step: {e}");
            std::process::exit(1);
        }
    };
    if args.what == Want::Plan {
        println!("canary {} {}", plan.canary.0, plan.canary.1);
        for (button, pressed, x, y) in &plan.buttons {
            let button = serde_json::to_string(button)?;
            println!("button {button} {pressed} {x} {y}");
        }
        return Ok(());
    }
    let started = Instant::now();
    let limit = (args.timeout > 0.0).then(|| Duration::from_secs_f64(args.timeout));
    let mut checked_pid = Instant::now() - Duration::from_secs(1);
    loop {
        match judge(&read(&args.probe), args.what, &plan) {
            Verdict::Met(message) => {
                println!("{message}");
                return Ok(());
            }
            Verdict::Failed(message) => {
                println!("{message}");
                std::process::exit(1);
            }
            Verdict::Waiting => {}
        }
        if let Some(pid) = args.pid {
            if checked_pid.elapsed() >= Duration::from_millis(200) {
                checked_pid = Instant::now();
                if !alive(pid) {
                    // It may have written its last words just before going.
                    if let Verdict::Failed(message) = judge(&read(&args.probe), args.what, &plan) {
                        println!("{message}");
                    } else {
                        println!("the test window closed");
                    }
                    std::process::exit(1);
                }
            }
        }
        if limit.is_some_and(|l| started.elapsed() >= l) {
            println!(
                "timed out after {} s waiting for {:?}",
                args.timeout, args.what
            );
            std::process::exit(2);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::Placement;
    use control::display::Display;

    fn script(json: &str) -> Vec<Step> {
        serde_json::from_str(json).unwrap()
    }

    const GOOD: &str = r#"[
        {"do":"request"},{"do":"wait","ms":500},
        {"do":"input","event":{"kind":"move","x":0.3,"y":0.4}},
        {"do":"input","event":{"kind":"move","x":0.5,"y":0.4}},
        {"do":"gate","file":"g1"},
        {"do":"input","event":{"kind":"button","button":"left","pressed":true,"x":0.5,"y":0.5}},
        {"do":"input","event":{"kind":"button","button":"left","pressed":false,"x":0.5,"y":0.5}},
        {"do":"input","event":{"kind":"scroll","dx":0,"dy":3,"unit":"line"}},
        {"do":"gate","file":"g2"},
        {"do":"type","text":"hi\n"},
        {"do":"input","event":{"kind":"key","code":"MetaLeft","pressed":true}},
        {"do":"release"}
    ]"#;

    #[test]
    fn plans_a_gated_script() {
        let p = plan(&script(GOOD)).unwrap();
        assert_eq!(p.canary, (0.5, 0.4));
        assert_eq!(
            p.buttons,
            [
                (Button::Left, true, 0.5, 0.5),
                (Button::Left, false, 0.5, 0.5)
            ]
        );
        assert!(p.keys_gate);
        assert_eq!(p.points.len(), 4);
    }

    #[test]
    fn the_shipped_script_has_the_right_shape() {
        let text = include_str!("../../../bench/control/inject/probe.json");
        let p = plan(&script(text)).unwrap();
        assert!(p.keys_gate);
        assert!(!p.buttons.is_empty());
    }

    #[test]
    fn refuses_scripts_that_send_risky_input_early() {
        let cases = [
            // No gate at all.
            r#"[{"do":"request"},{"do":"input","event":{"kind":"move","x":0.5,"y":0.5}}]"#,
            // A gate before any move.
            r#"[{"do":"request"},{"do":"gate","file":"g"}]"#,
            // A click before the first gate.
            r#"[{"do":"input","event":{"kind":"move","x":0.5,"y":0.5}},
                {"do":"input","event":{"kind":"button","button":"left","pressed":true,"x":0.5,"y":0.5}},
                {"do":"gate","file":"g"}]"#,
            // Text before the second gate.
            r#"[{"do":"input","event":{"kind":"move","x":0.5,"y":0.5}},{"do":"gate","file":"g"},
                {"do":"type","text":"x"}]"#,
            // A key after the only gate.
            r#"[{"do":"input","event":{"kind":"move","x":0.5,"y":0.5}},{"do":"gate","file":"g"},
                {"do":"input","event":{"kind":"key","code":"KeyA","pressed":true}}]"#,
            // Three gates.
            r#"[{"do":"input","event":{"kind":"move","x":0.5,"y":0.5}},{"do":"gate","file":"a"},
                {"do":"gate","file":"b"},{"do":"gate","file":"c"}]"#,
        ];
        for case in cases {
            assert!(plan(&script(case)).is_err(), "{case}");
        }
    }

    fn ready(frame: Frame) -> ProbeLine {
        ProbeLine::Ready {
            ready: true,
            display: Display {
                id: 1,
                x: 0.0,
                y: 0.0,
                width: 1512.0,
                height: 982.0,
                main: true,
            },
            frame,
            key: true,
            frontmost: true,
        }
    }

    const NOTCHED: Frame = Frame {
        x: 0.0,
        y: 0.033,
        width: 1.0,
        height: 0.967,
    };

    fn ev(e: InputEvent) -> ProbeLine {
        ProbeLine::Event(e)
    }

    fn mv(x: f64, y: f64) -> ProbeLine {
        ev(InputEvent::Move { x, y })
    }

    fn click(pressed: bool, x: f64, y: f64) -> ProbeLine {
        ev(InputEvent::Button {
            button: Button::Left,
            pressed,
            x,
            y,
        })
    }

    #[test]
    fn nothing_is_met_before_the_window_is_ready() {
        let p = plan(&script(GOOD)).unwrap();
        for want in [Want::Ready, Want::Canary, Want::Buttons, Want::Watch] {
            assert_eq!(judge(&[], want, &p), Verdict::Waiting);
        }
        assert!(matches!(
            judge(&[ready(NOTCHED)], Want::Ready, &p),
            Verdict::Met(_)
        ));
        let not_ready = ProbeLine::NotReady {
            not_ready: "not the key window".into(),
        };
        assert!(matches!(
            judge(&[not_ready], Want::Ready, &p),
            Verdict::Failed(_)
        ));
    }

    #[test]
    fn a_script_aiming_off_the_window_is_refused_when_ready() {
        let p = Plan {
            points: vec![(0.5, 0.01)],
            ..plan(&script(GOOD)).unwrap()
        };
        assert!(matches!(
            judge(&[ready(NOTCHED)], Want::Ready, &p),
            Verdict::Failed(_)
        ));
    }

    #[test]
    fn the_canary_must_arrive_where_it_was_aimed() {
        let p = plan(&script(GOOD)).unwrap();
        let mut lines = vec![ready(NOTCHED), mv(0.3, 0.4)];
        assert_eq!(judge(&lines, Want::Canary, &p), Verdict::Waiting);
        lines.push(mv(0.9, 0.9));
        assert_eq!(
            judge(&lines, Want::Canary, &p),
            Verdict::Waiting,
            "elsewhere"
        );
        lines.push(mv(0.505, 0.395));
        assert!(matches!(judge(&lines, Want::Canary, &p), Verdict::Met(_)));
    }

    #[test]
    fn clicks_must_arrive_in_order_and_in_place() {
        let p = plan(&script(GOOD)).unwrap();
        let mut lines = vec![ready(NOTCHED), mv(0.5, 0.4), click(true, 0.5, 0.5)];
        assert_eq!(judge(&lines, Want::Buttons, &p), Verdict::Waiting);
        lines.push(click(false, 0.51, 0.49));
        assert!(matches!(judge(&lines, Want::Buttons, &p), Verdict::Met(_)));
        let wrong = vec![ready(NOTCHED), click(true, 0.2, 0.5)];
        assert!(matches!(
            judge(&wrong, Want::Buttons, &p),
            Verdict::Failed(_)
        ));
    }

    #[test]
    fn losing_focus_closing_or_stray_keys_fail_every_wait() {
        let p = plan(&script(GOOD)).unwrap();
        let lost = ProbeLine::LostFocus {
            lost_focus: Placement {
                on_target: true,
                key: false,
                frontmost: false,
                frame: Some(NOTCHED),
            },
        };
        let key = ev(InputEvent::Key {
            code: "KeyA".into(),
            pressed: true,
            text: Some("a".into()),
        });
        for want in [Want::Ready, Want::Canary, Want::Buttons, Want::Watch] {
            for bad in [&lost, &ProbeLine::Closed { closed: true }] {
                let lines = vec![ready(NOTCHED), mv(0.5, 0.4), bad.clone()];
                assert!(
                    matches!(judge(&lines, want, &p), Verdict::Failed(_)),
                    "{want:?} {bad:?}"
                );
            }
        }
        for want in [Want::Ready, Want::Canary, Want::Buttons] {
            let lines = vec![ready(NOTCHED), mv(0.5, 0.4), key.clone()];
            assert!(matches!(judge(&lines, want, &p), Verdict::Failed(_)));
        }
        // While watching the keys go out, keys are expected.
        let lines = vec![ready(NOTCHED), key];
        assert_eq!(judge(&lines, Want::Watch, &p), Verdict::Waiting);
    }
}
