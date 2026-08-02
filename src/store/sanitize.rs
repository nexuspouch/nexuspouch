//! Sensitive scrubbing for session-history indexing (SESSION_HISTORY_DESIGN §4).
//!
//! Applies to FTS/vector index text only; raw store files stay untouched.

/// Default `strict` scrubbing: redact common secret / PII shaped tokens.
pub fn scrub_strict(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for line in input.lines() {
        out.push_str(&scrub_line(line));
        out.push('\n');
    }
    if !input.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    out
}

fn scrub_line(line: &str) -> String {
    let mut s = line.to_string();
    // Bearer / API tokens
    s = redact_regexish(&s, "sk-", 8, 40);
    s = redact_regexish(&s, "ghp_", 4, 40);
    s = redact_regexish(&s, "xox", 3, 40);
    // env-style KEY=value
    if let Some(eq) = s.find('=') {
        let key = s[..eq].trim().to_ascii_lowercase();
        if key.contains("token")
            || key.contains("secret")
            || key.contains("password")
            || key.contains("api_key")
            || key.ends_with("_key")
        {
            return format!("{}=<redacted>", s[..eq].trim());
        }
    }
    // emails
    if s.contains('@') {
        let mut buf = String::new();
        for tok in s.split_whitespace() {
            if tok.contains('@') && tok.contains('.') {
                buf.push_str("<redacted-email>");
            } else {
                buf.push_str(tok);
            }
            buf.push(' ');
        }
        s = buf.trim_end().to_string();
    }
    // absolute home paths (best-effort)
    if let Some(idx) = s.find("/Users/") {
        s = format!("{}<redacted-path>{}", &s[..idx], strip_path_tail(&s[idx..]));
    } else if let Some(idx) = s.find("/home/") {
        s = format!("{}<redacted-path>{}", &s[..idx], strip_path_tail(&s[idx..]));
    }
    s
}

fn strip_path_tail(s: &str) -> &str {
    s.find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
        .map(|i| &s[i..])
        .unwrap_or("")
}

fn redact_regexish(s: &str, prefix: &str, min_rest: usize, max_rest: usize) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(prefix) {
        out.push_str(&rest[..i]);
        let after = &rest[i + prefix.len()..];
        let n = after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .count();
        if n >= min_rest && n <= max_rest {
            out.push_str("<redacted-token>");
            rest = &after[after.char_indices().nth(n).map(|(i, _)| i).unwrap_or(after.len())..];
        } else {
            out.push_str(prefix);
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubs_token_email_and_path() {
        let raw = "Authorization: Bearer sk-abcdefghijklmnop\nuser@example.com\npath=/Users/me/secret/file";
        let out = scrub_strict(raw);
        assert!(!out.contains("sk-abcdefghijklmnop"));
        assert!(out.contains("<redacted-token>") || out.contains("<redacted"));
        assert!(out.contains("<redacted-email>") || !out.contains("user@example.com"));
        assert!(out.contains("<redacted-path>"));
    }
}
