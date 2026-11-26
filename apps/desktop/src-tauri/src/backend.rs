//! Which account backend this app talks to: `TWINCURSOR_SUPABASE_URL` and
//! `TWINCURSOR_SUPABASE_KEY` from the environment, else what the build baked in (`build.rs`),
//! else none, and the app shows no account features.

use account::Backend;

pub const URL_ENV: &str = "TWINCURSOR_SUPABASE_URL";
pub const KEY_ENV: &str = "TWINCURSOR_SUPABASE_KEY";

mod baked {
    include!(concat!(env!("OUT_DIR"), "/backend.rs"));
}

fn filled(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

/// Each value from the strongest source that has one. `Ok(None)` when nothing names a backend.
pub fn resolve(
    env_url: Option<String>,
    env_key: Option<String>,
    baked_url: Option<&str>,
    baked_key: Option<&str>,
) -> Result<Option<Backend>, String> {
    let url = filled(env_url).or_else(|| filled(baked_url.map(str::to_owned)));
    let key = filled(env_key).or_else(|| filled(baked_key.map(str::to_owned)));
    match (url, key) {
        (None, None) => Ok(None),
        (Some(url), Some(key)) => Backend::new(&url, &key)
            .map(Some)
            .map_err(|e| e.to_string()),
        _ => Err(format!(
            "The account backend needs both {URL_ENV} and {KEY_ENV} (or both baked into the build)."
        )),
    }
}

/// The backend for this run.
pub fn from_environment() -> Result<Option<Backend>, String> {
    resolve(
        std::env::var(URL_ENV).ok(),
        std::env::var(KEY_ENV).ok(),
        baked::URL,
        baked::KEY,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCAL: &str = "http://127.0.0.1:54421";
    const HOSTED: &str = "https://abc.supabase.co";

    #[test]
    fn the_environment_beats_the_build() {
        let b = resolve(
            Some(LOCAL.into()),
            Some("k1".into()),
            Some(HOSTED),
            Some("k2"),
        )
        .unwrap()
        .unwrap();
        assert_eq!((b.url(), b.key()), (LOCAL, "k1"));
        let b = resolve(None, Some(" ".into()), Some(HOSTED), Some("k2"))
            .unwrap()
            .unwrap();
        assert_eq!((b.url(), b.key()), (HOSTED, "k2"));
        // Each value separately, as tc-engine resolves its server.
        let b = resolve(Some(LOCAL.into()), None, Some(HOSTED), Some("k2"))
            .unwrap()
            .unwrap();
        assert_eq!((b.url(), b.key()), (LOCAL, "k2"));
    }

    #[test]
    fn nothing_named_means_not_set_up() {
        assert_eq!(resolve(None, None, None, None), Ok(None));
        assert_eq!(
            resolve(Some("".into()), Some(" ".into()), None, None),
            Ok(None)
        );
    }

    #[test]
    fn half_a_backend_or_a_bad_one_is_an_error() {
        assert!(resolve(Some(LOCAL.into()), None, None, None)
            .unwrap_err()
            .contains("needs both"));
        assert!(resolve(None, None, None, Some("k"))
            .unwrap_err()
            .contains("needs both"));
        assert!(resolve(
            Some("http://example.test".into()),
            Some("k".into()),
            None,
            None
        )
        .unwrap_err()
        .contains("https://"));
    }
}
