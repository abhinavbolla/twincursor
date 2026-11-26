//! The app's small settings file: where the call bar was last left.
//!
//! Saved as JSON in the app's config directory, readable only by this user. It once also held
//! server overrides (address, API key and secret) typed into a Settings page; account calls get
//! their server from the call pass, so that page is gone. A file from then still loads, and the
//! next save leaves those fields out.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// Where the call bar was last left: its top-left corner, in whole points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bar_position: Option<(i32, i32)>,
}

impl Settings {
    /// Reads the file; a missing file means nothing saved.
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

    /// Writes the file, readable and writable by this user only.
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("twincursor-settings-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("settings.json")
    }

    #[test]
    fn the_bar_position_round_trips() {
        let path = temp_path("bar");
        assert_eq!(Settings::load(&path).unwrap(), Settings::default());
        let mut settings = Settings {
            bar_position: Some((-300, 712)),
        };
        settings.save(&path).unwrap();
        assert_eq!(
            Settings::load(&path).unwrap().bar_position,
            Some((-300, 712))
        );
        settings.bar_position = None;
        settings.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("bar_position"), "{text}");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_file_with_old_server_overrides_loads_and_loses_them_on_save() {
        let path = temp_path("old");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"url":"wss://a.test","api_key":"k","api_secret":"old-secret","bar_position":[4,5]}"#,
        )
        .unwrap();
        let settings = Settings::load(&path).unwrap();
        assert_eq!(settings.bar_position, Some((4, 5)));
        settings.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("old-secret") && !text.contains("url"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn only_this_user_can_read_the_file() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_path("mode");
        let settings = Settings {
            bar_position: Some((1, 2)),
        };
        settings.save(&path).unwrap();
        settings.save(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
