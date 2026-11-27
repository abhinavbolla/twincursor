//! `tc-account`: the account backend from a terminal, for benches and for trying a backend by
//! hand. A development tool; the app doesn't bundle it.
//!
//! The backend comes from `TWINCURSOR_SUPABASE_URL` and `TWINCURSOR_SUPABASE_KEY`, the session
//! from `--session FILE` (an owner-only file holding a bearer credential). Answers are JSON on
//! stdout; a failure is `{"error": "<sentence>", "kind": "<word>"}` on stdout and exit status 1.
//! No token is ever printed, except a call pass by `pass --print-env`, which exists so a script
//! can hand the pass to `tc-engine`.

use std::path::PathBuf;
use std::process::ExitCode;

use account::{Account, Backend, Cancel, Error, FileStore, Secret, SignInOptions};
use clap::{ArgGroup, Parser, Subcommand};
use serde_json::{json, Value};

const URL_ENV: &str = "TWINCURSOR_SUPABASE_URL";
const KEY_ENV: &str = "TWINCURSOR_SUPABASE_KEY";
const PASSWORD_ENV: &str = "TC_ACCOUNT_PASSWORD";

#[derive(Parser)]
#[command(
    name = "tc-account",
    about = "Sign in to a TwinCursor backend and call it, for benches and trying a backend by hand",
    after_help = "The backend comes from TWINCURSOR_SUPABASE_URL and TWINCURSOR_SUPABASE_KEY (its public key). \
Answers are JSON on stdout; failures are {\"error\", \"kind\"} with exit status 1. The session file holds a \
bearer credential and is written readable by you only."
)]
struct Cli {
    /// Where the session is kept.
    #[arg(long, value_name = "FILE")]
    session: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Sign in with GitHub in the system browser (listens on 127.0.0.1:53682 for the callback).
    SignIn,
    /// Sign in with an email and the password in TC_ACCOUNT_PASSWORD. A test hook: only local
    /// stacks turn email sign-in on; a hosted backend offers GitHub alone.
    PasswordSignIn {
        #[arg(long)]
        email: String,
    },
    /// Your profile, and whether you may start calls on this server (can_host).
    Whoami,
    /// You (with can_host), contacts, the call you host, rings and knocks. --hosting names the
    /// call you are sharing, which is the host's heartbeat.
    Inbox {
        #[arg(long, value_name = "CALL")]
        hosting: Option<String>,
    },
    /// Start a call you host (ends your earlier one). Prints its id and code.
    StartCall,
    /// Ask to join the call with this code.
    Knock { code: String },
    /// Let someone knocking on your call in, or turn them away.
    #[command(group(ArgGroup::new("answer").required(true).args(["let_in", "turn_away"])))]
    AnswerKnock {
        call: String,
        user: String,
        #[arg(long)]
        let_in: bool,
        #[arg(long)]
        turn_away: bool,
    },
    /// Ring a contact (starts a call).
    Ring { contact: String },
    /// Answer a ring.
    #[command(group(ArgGroup::new("answer").required(true).args(["accept", "decline"])))]
    AnswerRing {
        call: String,
        #[arg(long)]
        accept: bool,
        #[arg(long)]
        decline: bool,
    },
    /// Leave a call you were let in to, or stop knocking.
    Leave { call: String },
    /// End a call you host.
    End { call: String },
    /// Remove someone from your contacts.
    RemoveContact { contact: String },
    /// Ask whoever runs the server to let you start calls (an invite-only server). Asking again
    /// changes nothing.
    AskToHost,
    /// A call pass for a call. Prints who it is for, without the token.
    Pass {
        call: String,
        /// Print TWINCURSOR_SERVER_URL=… and TWINCURSOR_PASS=… lines for a script to `eval`
        /// before starting tc-engine. The only way this tool ever prints a pass.
        #[arg(long)]
        print_env: bool,
    },
    /// Sign out: ends the session on the server and removes the session file.
    SignOut,
    /// Print the sign-in page's URL, with a throwaway challenge, without listening or opening a
    /// browser. For checking a backend accepts it.
    #[command(hide = true)]
    AuthorizeUrl,
}

fn backend() -> Result<Backend, Error> {
    let var = |name| std::env::var(name).unwrap_or_default();
    let (url, key) = (var(URL_ENV), var(KEY_ENV));
    if url.trim().is_empty() || key.trim().is_empty() {
        return Err(Error::BadBackend(
            "Set TWINCURSOR_SUPABASE_URL and TWINCURSOR_SUPABASE_KEY to the backend's URL and public key.",
        ));
    }
    Backend::new(&url, &key)
}

/// Quotes for a POSIX shell.
fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

fn open_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let opener = "/usr/bin/open";
    #[cfg(not(target_os = "macos"))]
    let opener = "xdg-open";
    std::process::Command::new(opener)
        .arg(url)
        .status()
        .map_err(|e| e.to_string())
        .and_then(|s| {
            s.success()
                .then_some(())
                .ok_or_else(|| "the opener failed".into())
        })
}

enum Output {
    Json(Value),
    Lines(String),
}

async fn run(cli: Cli) -> Result<Output, Error> {
    let account = Account::new(backend()?, FileStore::new(&cli.session))?;
    let json = |v: Value| Ok(Output::Json(v));
    match cli.cmd {
        Cmd::SignIn => {
            let cancel = Cancel::new();
            let c = cancel.clone();
            tokio::spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    c.cancel();
                }
            });
            eprintln!("Finish signing in in your browser… (Ctrl-C cancels)");
            account
                .sign_in(SignInOptions::default(), open_browser, &cancel)
                .await?;
            json(json!({ "signed_in": true, "user_id": account.user_id().await }))
        }
        Cmd::PasswordSignIn { email } => {
            let password = std::env::var(PASSWORD_ENV)
                .map_err(|_| Error::BadBackend("Put the password in TC_ACCOUNT_PASSWORD."))?;
            account
                .password_sign_in(&email, &Secret::new(password))
                .await?;
            json(json!({ "signed_in": true, "user_id": account.user_id().await }))
        }
        Cmd::Whoami => to(&account.inbox(None).await?.me),
        Cmd::Inbox { hosting } => to(&account.inbox(hosting.as_deref()).await?),
        Cmd::StartCall => to(&account.start_call().await?),
        Cmd::Knock { code } => to(&account.knock(&code).await?),
        Cmd::AnswerKnock {
            call, user, let_in, ..
        } => {
            let state = account.answer_knock(&call, &user, let_in).await?;
            json(json!({ "state": state }))
        }
        Cmd::Ring { contact } => to(&account.ring(&contact).await?),
        Cmd::AnswerRing { call, accept, .. } => {
            let state = account.answer_ring(&call, accept).await?;
            json(json!({ "state": state }))
        }
        Cmd::Leave { call } => {
            account.leave_call(&call).await?;
            json(json!({ "ok": true }))
        }
        Cmd::End { call } => {
            account.end_call(&call).await?;
            json(json!({ "ok": true }))
        }
        Cmd::RemoveContact { contact } => {
            account.remove_contact(&contact).await?;
            json(json!({ "ok": true }))
        }
        Cmd::AskToHost => {
            account.ask_to_host().await?;
            json(json!({ "ok": true }))
        }
        Cmd::Pass { call, print_env } => {
            let pass = account.call_pass(&call).await?;
            if print_env {
                return Ok(Output::Lines(format!(
                    "TWINCURSOR_SERVER_URL={}\nTWINCURSOR_PASS={}\n",
                    sh_quote(&pass.url),
                    sh_quote(pass.token.expose())
                )));
            }
            json(json!({
                "url": pass.url, "room": pass.room, "identity": pass.identity,
                "name": pass.name, "role": pass.role,
            }))
        }
        Cmd::AuthorizeUrl => {
            let pkce = account::Pkce::new()?;
            let redirect = format!(
                "http://127.0.0.1:{}{}",
                account::CALLBACK_PORT,
                account::CALLBACK_PATH
            );
            json(json!({ "url": account.authorize_url(&redirect, &pkce) }))
        }
        Cmd::SignOut => {
            account.sign_out().await?;
            json(json!({ "signed_in": false }))
        }
    }
}

/// One of the crate's answer types as JSON. None of them holds a token: `Secret` can't be
/// serialized.
fn to<T: serde::Serialize>(value: &T) -> Result<Output, Error> {
    Ok(Output::Json(
        serde_json::to_value(value).map_err(|_| Error::BadAnswer)?,
    ))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("tc-account: no async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(cli)) {
        Ok(Output::Json(value)) => {
            println!("{value}");
            ExitCode::SUCCESS
        }
        Ok(Output::Lines(text)) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            println!("{}", json!({ "error": e.to_string(), "kind": e.kind() }));
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_survives_the_shell() {
        assert_eq!(sh_quote("ws://127.0.0.1:7880"), "'ws://127.0.0.1:7880'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn answers_need_one_choice() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
        for args in [
            &["tc-account", "--session", "s", "answer-knock", "c", "u"][..],
            &[
                "tc-account",
                "--session",
                "s",
                "answer-knock",
                "c",
                "u",
                "--let-in",
                "--turn-away",
            ],
            &["tc-account", "--session", "s", "answer-ring", "c"],
        ] {
            assert!(Cli::try_parse_from(args).is_err(), "{args:?}");
        }
        assert!(Cli::try_parse_from([
            "tc-account",
            "--session",
            "s",
            "answer-ring",
            "c",
            "--decline"
        ])
        .is_ok());
    }
}
