//! Where the backend is.

use crate::Error;

/// A Supabase project: its URL and its public ("anon" or publishable) key. Both are public by
/// design; the key only identifies the project, and the database's own rules decide what a
/// signed-in person may do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backend {
    url: String,
    key: String,
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "[::1]")
}

impl Backend {
    /// Checks the URL: `https://` only, except `http://` on 127.0.0.1 or localhost for a local
    /// stack. No user name, password, query or fragment. A trailing `/` is dropped.
    pub fn new(url: &str, key: &str) -> Result<Backend, Error> {
        let url = url.trim();
        let key = key.trim();
        let parsed = reqwest::Url::parse(url)
            .map_err(|_| Error::BadBackend("The backend address isn't a URL."))?;
        let host = parsed.host_str().unwrap_or_default();
        match parsed.scheme() {
            "https" if !host.is_empty() => {}
            "http" if is_loopback(host) => {}
            _ => {
                return Err(Error::BadBackend(
                    "The backend address must start with https:// (http:// only for 127.0.0.1 or localhost).",
                ))
            }
        }
        if !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(Error::BadBackend(
                "The backend address can't carry a user name, a query or a fragment.",
            ));
        }
        if key.is_empty() {
            return Err(Error::BadBackend("The backend's public key is empty."));
        }
        let mut url = parsed.to_string();
        while url.ends_with('/') {
            url.pop();
        }
        Ok(Backend {
            url,
            key: key.to_owned(),
        })
    }

    /// The project's URL, without a trailing `/`.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The public key, sent as `apikey` with every request.
    pub fn key(&self) -> &str {
        &self.key
    }

    pub(crate) fn endpoint(&self, path: &str) -> String {
        format!("{}{path}", self.url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_anywhere_http_only_on_this_machine() {
        let b = Backend::new(" https://abc.supabase.co/ ", " key ").unwrap();
        assert_eq!((b.url(), b.key()), ("https://abc.supabase.co", "key"));
        for url in [
            "http://127.0.0.1:54421",
            "http://localhost:54421/",
            "http://[::1]:54421",
        ] {
            assert!(Backend::new(url, "k").is_ok(), "{url}");
        }
        assert_eq!(
            Backend::new("https://example.test/base/", "k")
                .unwrap()
                .endpoint("/rest/v1/rpc/inbox"),
            "https://example.test/base/rest/v1/rpc/inbox"
        );
    }

    #[test]
    fn anything_else_is_refused() {
        for url in [
            "http://abc.supabase.co",
            "http://192.168.1.2:54421",
            "http://127.0.0.1.example.test",
            "ftp://example.test",
            "ws://127.0.0.1:54421",
            "example.test",
            "",
        ] {
            let e = Backend::new(url, "k").unwrap_err();
            assert!(matches!(e, Error::BadBackend(_)), "{url}: {e}");
        }
        for url in [
            "https://user:pw@example.test",
            "https://example.test/?a=1",
            "https://example.test/#x",
        ] {
            assert_eq!(
                Backend::new(url, "k").unwrap_err().to_string(),
                "The backend address can't carry a user name, a query or a fragment.",
            );
        }
        assert_eq!(
            Backend::new("https://example.test", " ")
                .unwrap_err()
                .to_string(),
            "The backend's public key is empty."
        );
    }
}
