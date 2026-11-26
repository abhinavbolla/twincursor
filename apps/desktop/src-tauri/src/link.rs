//! The call link: `twincursor://call/<code>`, where the code is the 32 lowercase hex characters
//! the backend gave the call. The link names no server: the app knows its own backend.
//!
//! Parsing is strict. It takes the whole link or the bare code, with whitespace around it, and
//! nothing else: no other scheme or path, no query, no uppercase, no extra characters.

/// What every call link starts with.
pub const PREFIX: &str = "twincursor://call/";
/// How many hex characters a code has.
pub const CODE_LEN: usize = 32;
/// Text longer than this isn't looked at (a pasted page, say): it can't be a link.
pub const MAX_TEXT: usize = 256;

/// The link for a call's code.
pub fn make(code: &str) -> Result<String, LinkError> {
    check_code(code)?;
    Ok(format!("{PREFIX}{code}"))
}

/// The code in `text`: a call link or a bare code, with whitespace around it.
pub fn parse(text: &str) -> Result<String, LinkError> {
    if text.len() > MAX_TEXT {
        return Err(LinkError::NotALink);
    }
    let text = text.trim();
    if text.is_empty() {
        return Err(LinkError::Empty);
    }
    let code = match text.strip_prefix(PREFIX) {
        Some(code) => code,
        None if text.contains("://") || text.contains('/') => return Err(LinkError::NotALink),
        None => text,
    };
    check_code(code)?;
    Ok(code.to_owned())
}

fn check_code(code: &str) -> Result<(), LinkError> {
    let hex = |c: char| c.is_ascii_digit() || ('a'..='f').contains(&c);
    if code.chars().count() == CODE_LEN && code.chars().all(hex) {
        Ok(())
    } else {
        Err(LinkError::BadCode)
    }
}

/// Why some text isn't a call link. Each reads as a sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkError {
    Empty,
    NotALink,
    BadCode,
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            LinkError::Empty => "There's no call link to join. Copy the link first.",
            LinkError::NotALink => "That isn't a TwinCursor call link.",
            LinkError::BadCode => "That call link is incomplete or mistyped.",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn makes_and_reads_back() {
        let link = make(CODE).unwrap();
        assert_eq!(link, format!("twincursor://call/{CODE}"));
        assert_eq!(parse(&link).unwrap(), CODE);
        assert_eq!(make("abc"), Err(LinkError::BadCode));
        assert_eq!(make(&CODE.to_uppercase()), Err(LinkError::BadCode));
    }

    #[test]
    fn takes_the_link_or_the_code_with_whitespace() {
        for text in [
            CODE.to_owned(),
            format!("  {CODE}\n"),
            format!("\t{PREFIX}{CODE}  \r\n"),
        ] {
            assert_eq!(parse(&text).unwrap(), CODE, "{text:?}");
        }
    }

    #[test]
    fn refuses_everything_else() {
        let cases = [
            ("", LinkError::Empty),
            ("   \n", LinkError::Empty),
            ("hello", LinkError::BadCode),
            (&CODE[..31], LinkError::BadCode),
            (&format!("{CODE}0"), LinkError::BadCode),
            (&CODE.to_uppercase(), LinkError::BadCode),
            (&format!("{}g", &CODE[..31]), LinkError::BadCode),
            (&format!("{PREFIX}{CODE}?x=1"), LinkError::BadCode),
            (&format!("{PREFIX}{CODE}/"), LinkError::BadCode),
            (&format!("{PREFIX}{CODE} extra"), LinkError::BadCode),
            (PREFIX, LinkError::BadCode),
            (
                &format!("https://example.test/call/{CODE}"),
                LinkError::NotALink,
            ),
            (&format!("TWINCURSOR://call/{CODE}"), LinkError::NotALink),
            (&format!("twincursor://room/{CODE}"), LinkError::NotALink),
            (&format!("twincursor://call//{CODE}"), LinkError::BadCode),
            (&format!("{CODE} {CODE}"), LinkError::BadCode),
            (&format!("x{PREFIX}{CODE}"), LinkError::NotALink),
            (&"a".repeat(MAX_TEXT + 1), LinkError::NotALink),
            // Look-alike digits and invisible characters are not hex.
            (&format!("{}\u{200b}", &CODE[..31]), LinkError::BadCode),
            (&format!("{}０", &CODE[..31]), LinkError::BadCode),
        ];
        for (text, want) in cases {
            assert_eq!(parse(text), Err(want), "{text:?}");
        }
    }

    #[test]
    fn errors_are_sentences() {
        for e in [LinkError::Empty, LinkError::NotALink, LinkError::BadCode] {
            assert!(e.to_string().ends_with('.'), "{e}");
        }
    }
}
