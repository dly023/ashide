use regex::{Captures, Regex};
use std::sync::OnceLock;

pub fn clean_text(text: impl AsRef<str>) -> String {
    sanitize_embedded_images(&redact(text.as_ref()))
}

pub fn redact(text: &str) -> String {
    let mut safe = text.to_owned();
    safe = github_token_re()
        .replace_all(&safe, "[REDACTED]")
        .into_owned();
    safe = openai_key_re()
        .replace_all(&safe, "[REDACTED]")
        .into_owned();
    safe = private_key_re()
        .replace_all(&safe, "[REDACTED_PRIVATE_KEY]")
        .into_owned();
    key_value_secret_re()
        .replace_all(&safe, |captures: &Captures<'_>| {
            format!(
                "{}[REDACTED]",
                captures.get(1).map(|m| m.as_str()).unwrap_or_default()
            )
        })
        .into_owned()
}

pub fn sanitize_embedded_images(text: &str) -> String {
    let clean = strip_input_image_prefix_before_data_url(text);
    image_data_url_re()
        .replace_all(&clean, |captures: &Captures<'_>| {
            let mime_type = captures
                .get(1)
                .map(|m| m.as_str())
                .unwrap_or("image/unknown");
            let payload = captures.get(2).map(|m| m.as_str()).unwrap_or_default();
            embedded_image_placeholder(mime_type, payload)
        })
        .into_owned()
}

fn embedded_image_placeholder(mime_type: &str, payload: &str) -> String {
    let clean_payload: String = payload
        .chars()
        .filter(|c| *c != '\r' && *c != '\n')
        .collect();
    let byte_count = base64_decoded_size(&clean_payload);
    let suffix = if byte_count > 0 {
        format!(", approx {}", format_bytes(byte_count))
    } else {
        String::new()
    };
    let image_type = mime_type
        .split_once('/')
        .map(|(_, subtype)| subtype)
        .unwrap_or("unknown")
        .to_ascii_uppercase();
    format!("[Image attachment not imported: embedded {image_type} data URL{suffix}]")
}

fn base64_decoded_size(payload: &str) -> usize {
    if payload.is_empty() {
        return 0;
    }
    let padding = payload.chars().rev().take_while(|c| *c == '=').count();
    payload
        .len()
        .saturating_mul(3)
        .saturating_div(4)
        .saturating_sub(padding)
}

fn format_bytes(size: usize) -> String {
    if size < 1024 {
        return format!("{size} B");
    }
    let mut value = size as f64;
    for unit in ["KB", "MB", "GB"] {
        value /= 1024.0;
        if value < 1024.0 || unit == "GB" {
            return format!("{value:.1} {unit}");
        }
    }
    format!("{value:.1} GB")
}

fn github_token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"gh[pousr]_[A-Za-z0-9_]{8,}").expect("valid github token regex"))
}

fn openai_key_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"sk-[A-Za-z0-9_-]{12,}").expect("valid api key regex"))
}

fn key_value_secret_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)(\b(?:token|api[_-]?key|secret|cookie|passwd|password|pwd|pass)\s*[=:]\s*['\"]?)([^\s'\"]+)"#)
            .expect("valid key/value secret regex")
    })
}

fn private_key_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----")
            .expect("valid private key regex")
    })
}

fn strip_input_image_prefix_before_data_url(text: &str) -> String {
    let needle = "input_image";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let Some(index) = rest.to_ascii_lowercase().find(needle) else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..index]);
        let after = &rest[index + needle.len()..];
        let whitespace_len = after
            .char_indices()
            .take_while(|(_, ch)| ch.is_whitespace())
            .map(|(idx, ch)| idx + ch.len_utf8())
            .last()
            .unwrap_or(0);
        if whitespace_len > 0 && after[whitespace_len..].starts_with("data:image/") {
            rest = &after[whitespace_len..];
        } else {
            out.push_str(&rest[index..index + needle.len()]);
            rest = after;
        }
    }
}

fn image_data_url_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"data:(image/[A-Za-z0-9.+-]+);base64,([A-Za-z0-9+/=\r\n]+)")
            .expect("valid image data URL regex")
    })
}
