//! The server settings a person can override: address, API key and API secret.
//!
//! What the build baked into `tc-engine` is the default (`tc-engine config` shows it, without the
//! secret). Overrides are saved as JSON in the app's config directory, readable only by this
//! user, and reach the engine through its environment, never its flags: other users of the Mac
//! can read a process's flags. The secret never goes back to the webview; it only learns whether
//! one is set.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// The environment variables `tc-engine` reads, strongest after its own flags.
pub const URL_ENV: &str = "TWINCURSOR_SERVER_URL";
pub const KEY_ENV: &str = "TWINCURSOR_API_KEY";
pub const SECRET_ENV: &str = "TWINCURSOR_API_SECRET";

/// The saved overrides. `None` means "use the build's".
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_secret: Option<String>,
    /// Where the call bar was last left: its top-left corner, in whole points. Not a server
    /// setting, and never shown to the webview; it lives here so the app has one settings file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bar_position: Option<(i32, i32)>,
}

impl std::fmt::Debug for Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Settings")
            .field("url", &self.url)
            .field("api_key", &self.api_key)
            .field("api_secret", &self.api_secret.as_ref().map(|_| "(set)"))
            .field("bar_position", &self.bar_position)
            .finish()
    }
}

/// What the webview may see of the settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettingsView {
    pub url: Option<String>,
    pub api_key: Option<String>,
    /// An overriding secret is saved. Its value never leaves this process.
    pub has_secret: bool,
}

/// What the webview asks to change. Empty text clears an override.
#[derive(Debug, Clone, Deserialize)]
pub struct SettingsChange {
    pub url: Option<String>,
    pub api_key: Option<String>,
    pub api_secret: SecretChange,
}

/// The secret is written, never read back, so "leave it as it is" needs saying.
#[derive(Clone, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum SecretChange {
    Keep,
    Clear,
    Set { value: String },
}

impl std::fmt::Debug for SecretChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SecretChange::Keep => "Keep",
            SecretChange::Clear => "Clear",
            SecretChange::Set { .. } => "Set(..)",
        })
    }
}

fn filled(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

impl Settings {
    /// Reads the saved overrides; a missing file means none.
    pub fn load(path: &Path) -> Result<Settings, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| {
                format!(
                    "The settings file {} couldn't be read: {e}.",
                    path.display()
                )
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(e) => Err(format!(
                "The settings file {} couldn't be read: {e}.",
                path.display()
            )),
        }
    }

    /// Writes the overrides, readable and writable by this user only.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let fail =
            |e: std::io::Error| format!("Settings couldn't be saved to {}: {e}.", path.display());
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(fail)?;
        }
        let text = serde_json::to_string_pretty(self).expect("settings serialize");
        let tmp = path.with_extension("json.tmp");
        let _ = std::fs::remove_file(&tmp);
        {
            use std::io::Write;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&tmp).map_err(fail)?;
            file.write_all(text.as_bytes()).map_err(fail)?;
            file.sync_all().map_err(fail)?;
        }
        std::fs::rename(&tmp, path).map_err(fail)
    }

    pub fn apply(&mut self, change: SettingsChange) {
        self.url = filled(change.url);
        self.api_key = filled(change.api_key);
        match change.api_secret {
            SecretChange::Keep => {}
            SecretChange::Clear => self.api_secret = None,
            SecretChange::Set { value } => self.api_secret = filled(Some(value)),
        }
    }

    pub fn view(&self) -> SettingsView {
        SettingsView {
            url: self.url.clone(),
            api_key: self.api_key.clone(),
            has_secret: self.api_secret.is_some(),
        }
    }

    /// The engine's environment for these overrides. Only a room-code call takes them, and
    /// those start only from scripts now, which leave saved settings out; account calls get
    /// their server from the call pass.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn env(&self) -> Vec<(String, String)> {
        [
            (URL_ENV, &self.url),
            (KEY_ENV, &self.api_key),
            (SECRET_ENV, &self.api_secret),
        ]
        .into_iter()
        .filter_map(|(name, value)| value.clone().map(|v| (name.to_owned(), v)))
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "s3cr3t-value-that-must-not-leak";

    fn temp_path(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("twincursor-settings-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("settings.json")
    }

    fn change(secret: SecretChange) -> SettingsChange {
        SettingsChange {
            url: Some(" wss://example.test ".into()),
            api_key: Some("key-1".into()),
            api_secret: secret,
        }
    }

    #[test]
    fn round_trip_without_leaking_the_secret_to_the_view() {
        let path = temp_path("round-trip");
        let mut settings = Settings::load(&path).unwrap();
        assert_eq!(settings, Settings::default());
        settings.apply(change(SecretChange::Set {
            value: SECRET.into(),
        }));
        settings.save(&path).unwrap();

        let again = Settings::load(&path).unwrap();
        assert_eq!(again, settings);
        assert_eq!(again.url.as_deref(), Some("wss://example.test"));
        assert_eq!(again.api_secret.as_deref(), Some(SECRET));

        let view = again.view();
        assert!(view.has_secret);
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains(SECRET), "{json}");
        assert!(!format!("{again:?}").contains(SECRET));

        // Keep leaves the secret; clear removes it; empty text clears the others.
        let mut kept = again.clone();
        kept.apply(change(SecretChange::Keep));
        assert_eq!(kept.api_secret.as_deref(), Some(SECRET));
        kept.apply(SettingsChange {
            url: Some("  ".into()),
            api_key: None,
            api_secret: SecretChange::Clear,
        });
        assert_eq!(kept, Settings::default());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn only_this_user_can_read_the_file() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_path("mode");
        let mut settings = Settings::default();
        settings.apply(change(SecretChange::Set {
            value: SECRET.into(),
        }));
        settings.save(&path).unwrap();
        settings.save(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn overrides_become_the_engines_environment() {
        let mut settings = Settings::default();
        assert!(settings.env().is_empty());
        settings.apply(change(SecretChange::Set {
            value: SECRET.into(),
        }));
        let env = settings.env();
        assert!(env.contains(&(SECRET_ENV.into(), SECRET.into())));
        assert!(env.contains(&(URL_ENV.into(), "wss://example.test".into())));
        assert!(env.contains(&(KEY_ENV.into(), "key-1".into())));
    }

    #[test]
    fn the_bar_position_is_kept_alongside_and_survives_a_change() {
        let path = temp_path("bar");
        let mut settings = Settings {
            bar_position: Some((-300, 712)),
            ..Settings::default()
        };
        settings.save(&path).unwrap();
        let mut again = Settings::load(&path).unwrap();
        assert_eq!(again.bar_position, Some((-300, 712)));
        // Saving server overrides from the window leaves it alone.
        again.apply(change(SecretChange::Keep));
        assert_eq!(again.bar_position, Some((-300, 712)));
        settings.bar_position = None;
        settings.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("bar_position"), "{text}");
        // A file from before has none.
        assert_eq!(
            serde_json::from_str::<Settings>(r#"{"url":"wss://a.test"}"#)
                .unwrap()
                .bar_position,
            None
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_change_reads_from_the_webview() {
        let c: SettingsChange = serde_json::from_str(
            r#"{"url":null,"api_key":"k","api_secret":{"do":"set","value":"v"}}"#,
        )
        .unwrap();
        assert!(matches!(c.api_secret, SecretChange::Set { .. }));
        assert!(!format!("{c:?}").contains("\"v\""));
        let c: SettingsChange =
            serde_json::from_str(r#"{"url":null,"api_key":null,"api_secret":{"do":"keep"}}"#)
                .unwrap();
        assert!(matches!(c.api_secret, SecretChange::Keep));
    }
}
