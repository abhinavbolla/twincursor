//! The sharer's side of remote control in `tc-spike share --allow-control`.
//!
//! One thread owns the [`Controller`]: it takes link events and the sharer's answers, ticks the
//! deadman, and sends what the controller says to. Consent is a terminal prompt for now: a guest
//! asks, and the sharer types `y` to allow. The desktop app will replace only the prompt.

use std::io::BufRead;
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use control::{Config, Controller, Effect, Injector, LogInjector, RecordInjector};
use media::link::{ControlLink, LinkEvent, Outgoing};
use tokio::sync::mpsc::UnboundedSender;

const TICK: Duration = Duration::from_millis(20);

/// Where accepted input goes.
#[derive(Debug, Clone)]
pub enum Inject {
    /// Log each action, touching nothing.
    Log,
    /// Write each action as a line of JSON to this file (see `tc-spike replay`).
    Record(PathBuf),
}

impl std::str::FromStr for Inject {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.split_once(':') {
            None if s == "log" => Ok(Inject::Log),
            Some(("record", path)) if !path.is_empty() => Ok(Inject::Record(path.into())),
            _ => Err("expected `log` or `record:PATH`".into()),
        }
    }
}

enum In {
    Link(LinkEvent),
    Line(String),
    Stop,
}

pub struct Sharer {
    tx: Sender<In>,
    thread: JoinHandle<()>,
}

impl Sharer {
    /// Starts the control thread. Returns the link to hand to `publish::run`.
    pub fn start(inject: &Inject) -> anyhow::Result<(Self, ControlLink)> {
        let injector: Box<dyn Injector + Send> = match inject {
            Inject::Log => Box::new(LogInjector),
            Inject::Record(path) => Box::new(RecordInjector::new(std::fs::File::create(path)?)),
        };
        let (tx, rx) = mpsc::channel();
        let (link, outgoing) = {
            let tx = tx.clone();
            ControlLink::new(protocol::TOPIC, move |event| {
                let _ = tx.send(In::Link(event));
            })
        };
        {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for line in std::io::stdin().lock().lines() {
                    let Ok(line) = line else { break };
                    if tx.send(In::Line(line)).is_err() {
                        break;
                    }
                }
            });
        }
        let thread = std::thread::Builder::new()
            .name("control".into())
            .spawn(move || run(Controller::new(injector, Config::default()), &rx, &outgoing))?;
        println!(
            "Remote control is allowed to be requested. You'll be asked before anyone gets it."
        );
        Ok((Self { tx, thread }, link))
    }

    /// Ends any control and releases everything held, before the process exits.
    pub fn stop(self) {
        let _ = self.tx.send(In::Stop);
        let _ = self.thread.join();
    }
}

fn run<I: Injector>(
    mut controller: Controller<I>,
    rx: &mpsc::Receiver<In>,
    outgoing: &UnboundedSender<Outgoing>,
) {
    let mut bad_messages = 0u64;
    loop {
        let effects = match rx.recv_timeout(TICK) {
            Ok(In::Link(LinkEvent::Data { from, payload })) => match protocol::decode(&payload) {
                Ok(message) => controller.handle(&from, message, Instant::now()),
                Err(e) => {
                    bad_messages += 1;
                    tracing::debug!(%from, "bad control message: {e}");
                    vec![]
                }
            },
            Ok(In::Link(LinkEvent::Left(identity))) => controller.left(&identity),
            Ok(In::Link(_)) => vec![],
            Ok(In::Line(line)) => answer(&mut controller, line.trim()),
            Ok(In::Stop) | Err(RecvTimeoutError::Disconnected) => {
                apply(controller.revoke(), outgoing);
                break;
            }
            Err(RecvTimeoutError::Timeout) => vec![],
        };
        apply(effects, outgoing);
        controller.tick(Instant::now());
    }
    let counters = controller.counters();
    tracing::info!(?counters, bad_messages, "control stopped");
}

fn answer<I: Injector>(controller: &mut Controller<I>, line: &str) -> Vec<Effect> {
    if let Some(guest) = controller.requester().cloned() {
        if matches!(line, "y" | "Y" | "yes") {
            println!(
                "{} now controls this computer. Type r and Enter to take it back.",
                guest.name
            );
            controller.grant(Instant::now())
        } else {
            println!("Turned down {}.", guest.name);
            controller.deny()
        }
    } else if controller.controller().is_some() && line == "r" {
        println!("Control taken back.");
        controller.revoke()
    } else {
        vec![]
    }
}

fn apply(effects: Vec<Effect>, outgoing: &UnboundedSender<Outgoing>) {
    for effect in effects {
        match effect {
            Effect::Send { to, message } => {
                let _ = outgoing.send(Outgoing {
                    to: Some(to),
                    payload: protocol::encode(&message),
                    reliable: message.reliable(),
                });
            }
            Effect::Ask(guest) => {
                println!(
                    "\n{} ({}, {:?}) asks to control this computer. Allow? [y/N]",
                    guest.name, guest.identity, guest.platform
                );
            }
            Effect::Ended { identity, reason } => {
                println!("Control by {identity} ended: {reason:?}.");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inject_modes() {
        assert!(matches!("log".parse(), Ok(Inject::Log)));
        assert!(
            matches!("record:a.jsonl".parse(), Ok(Inject::Record(p)) if p == std::path::Path::new("a.jsonl"))
        );
        assert!("record:".parse::<Inject>().is_err());
        assert!("enigo".parse::<Inject>().is_err());
    }
}
