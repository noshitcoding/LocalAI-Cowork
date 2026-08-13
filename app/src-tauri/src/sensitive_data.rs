use serde_json::{Map, Value};

pub const REDACTED: &str = "[REDACTED]";
const TRUNCATED: &str = "[TRUNCATED]";
const MAX_REDACTION_DEPTH: usize = 16;

pub const MAX_LOG_SUMMARY_BYTES: usize = 8 * 1024;
pub const MAX_LOG_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_LOG_JSON_BYTES: usize = 256 * 1024;
pub const MAX_AUDIT_EVENT_BYTES: usize = 128 * 1024;

fn normalized_key(key: &str) -> String {
    key.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_sensitive_key(key: &str) -> bool {
    let key = normalized_key(key);
    key == "env"
        || key == "environment"
        || key == "headers"
        || key == "authorization"
        || key == "cookie"
        || key == "setcookie"
        || key == "auth"
        || key == "token"
        || key == "credentials"
        || key == "credential"
        || key == "configjson"
        || key.contains("apikey")
        || key.contains("accesstoken")
        || key.contains("refreshtoken")
        || key.contains("authtoken")
        || key.ends_with("token")
        || key.contains("password")
        || key.contains("passwd")
        || key.contains("clientsecret")
        || key.contains("privatekey")
        || key.contains("signingkey")
        || key.contains("webhookurl")
        || key.ends_with("secret")
}

fn redact_delimited_value(input: &str, marker: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let marker_lower = marker.to_ascii_lowercase();
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;

    while let Some(relative_start) = lower[cursor..].find(&marker_lower) {
        let start = cursor + relative_start;
        let value_start = start + marker.len();
        output.push_str(&input[cursor..value_start]);
        output.push_str(REDACTED);
        let mut value_end = value_start;
        for (offset, character) in input[value_start..].char_indices() {
            if character.is_whitespace() || matches!(character, '&' | '#' | ',' | ';' | '"' | '\'')
            {
                break;
            }
            value_end = value_start + offset + character.len_utf8();
        }
        if value_end == value_start {
            cursor = value_start;
            break;
        }
        cursor = value_end;
    }

    output.push_str(&input[cursor..]);
    output
}

fn redact_prefixed_token(input: &str, marker: &str, minimum_bytes: usize) -> String {
    let lower = input.to_ascii_lowercase();
    let marker_lower = marker.to_ascii_lowercase();
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;

    while let Some(relative_start) = lower[cursor..].find(&marker_lower) {
        let start = cursor + relative_start;
        let mut end = start;
        for (offset, character) in input[start..].char_indices() {
            if character.is_whitespace()
                || matches!(character, '&' | '#' | ',' | ';' | ':' | '"' | '\'')
            {
                break;
            }
            end = start + offset + character.len_utf8();
        }
        if end.saturating_sub(start) >= minimum_bytes {
            output.push_str(&input[cursor..start]);
            output.push_str(REDACTED);
        } else {
            output.push_str(&input[cursor..end]);
        }
        if end == cursor {
            break;
        }
        cursor = end;
    }

    output.push_str(&input[cursor..]);
    output
}

pub fn redact_text(input: &str) -> String {
    let mut output = input.to_string();
    for marker in [
        "bearer ",
        "api_key=",
        "apikey=",
        "access_token=",
        "refresh_token=",
        "token=",
        "password=",
        "passwd=",
        "client_secret=",
        "secret=",
        "signature=",
        "key=",
    ] {
        output = redact_delimited_value(&output, marker);
    }
    for (marker, minimum_bytes) in [("sk-", 11), ("ghp_", 12), ("xoxb-", 13), ("aiza", 12)] {
        output = redact_prefixed_token(&output, marker, minimum_bytes);
    }
    output
}

fn redact_value_at_depth(value: Value, depth: usize) -> Value {
    if depth > MAX_REDACTION_DEPTH {
        return Value::String(TRUNCATED.to_string());
    }

    match value {
        Value::Object(object) => Value::Object(redact_object(object, depth + 1)),
        Value::Array(entries) => Value::Array(
            entries
                .into_iter()
                .map(|entry| redact_value_at_depth(entry, depth + 1))
                .collect(),
        ),
        Value::String(text) => Value::String(redact_text(&text)),
        other => other,
    }
}

fn redact_object(object: Map<String, Value>, depth: usize) -> Map<String, Value> {
    object
        .into_iter()
        .map(|(key, value)| {
            let value = if is_sensitive_key(&key) {
                Value::String(REDACTED.to_string())
            } else {
                redact_value_at_depth(value, depth)
            };
            (key, value)
        })
        .collect()
}

pub fn redact_value(value: Value) -> Value {
    redact_value_at_depth(value, 0)
}

fn truncate_utf8(input: &str, max_bytes: usize) -> String {
    if input.len() <= max_bytes {
        return input.to_string();
    }
    if max_bytes == 0 {
        return String::new();
    }

    let marker = format!("\n{TRUNCATED}");
    if max_bytes <= marker.len() {
        return TRUNCATED.chars().take(max_bytes).collect::<String>();
    }

    let mut boundary = max_bytes - marker.len();
    while boundary > 0 && !input.is_char_boundary(boundary) {
        boundary -= 1;
    }

    let mut output = input[..boundary].to_string();
    output.push_str(&marker);
    output
}

pub fn redact_and_bound_text(input: &str, max_bytes: usize) -> String {
    truncate_utf8(&redact_text(input), max_bytes)
}

pub fn redact_and_bound_json_text(input: &str, max_bytes: usize) -> String {
    let Ok(value) = serde_json::from_str::<Value>(input) else {
        return redact_and_bound_text(input, max_bytes);
    };
    let serialized = redact_value(value).to_string();
    if serialized.len() <= max_bytes {
        return serialized;
    }

    let envelope_overhead = 128usize.min(max_bytes);
    let preview = truncate_utf8(&serialized, max_bytes.saturating_sub(envelope_overhead));
    let envelope = serde_json::json!({
        "$openCoworkTruncated": true,
        "originalBytes": serialized.len(),
        "preview": preview,
    })
    .to_string();

    if envelope.len() <= max_bytes {
        envelope
    } else if max_bytes >= 2 {
        "{}".to_string()
    } else {
        String::new()
    }
}

pub fn redact_and_bound_optional_text(input: Option<&str>, max_bytes: usize) -> Option<String> {
    input.map(|value| redact_and_bound_text(value, max_bytes))
}

pub fn redact_and_bound_optional_json(input: Option<&str>, max_bytes: usize) -> Option<String> {
    input.map(|value| redact_and_bound_json_text(value, max_bytes))
}

pub fn diagnostic_label(input: &str) -> String {
    let trimmed = input.trim();
    if !trimmed.is_empty()
        && trimmed.len() <= 128
        && trimmed
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._:-".contains(character))
    {
        trimmed.to_string()
    } else {
        REDACTED.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recursively_redacts_sensitive_keys_and_nested_environment_maps() {
        let input = serde_json::json!({
            "apiKey": "provider-secret",
            "nested": {
                "client_secret": "oauth-secret",
                "env": { "CUSTOM_VALUE": "environment-secret" },
                "safe": "visible"
            },
            "items": [{ "password": "password-secret" }]
        });

        let redacted = redact_value(input);

        assert_eq!(redacted["apiKey"], REDACTED);
        assert_eq!(redacted["nested"]["client_secret"], REDACTED);
        assert_eq!(redacted["nested"]["env"], REDACTED);
        assert_eq!(redacted["nested"]["safe"], "visible");
        assert_eq!(redacted["items"][0]["password"], REDACTED);
        let serialized = redacted.to_string();
        assert!(!serialized.contains("provider-secret"));
        assert!(!serialized.contains("environment-secret"));
    }

    #[test]
    fn redacts_authorization_and_query_tokens_inside_free_text() {
        let input = "request failed: Authorization: Bearer abc.def.ghi url=https://example.test/?token=url-secret&ok=1 sk-1234567890abcdef";
        let redacted = redact_text(input);

        assert!(!redacted.contains("abc.def.ghi"));
        assert!(!redacted.contains("url-secret"));
        assert!(!redacted.contains("sk-1234567890abcdef"));
        assert!(redacted.contains("ok=1"));
    }

    #[test]
    fn preserves_non_sensitive_operational_values() {
        let input = serde_json::json!({
            "status": 200,
            "durationMs": 42,
            "endpoint": "https://example.test/v1/models"
        });

        assert_eq!(redact_value(input.clone()), input);
    }

    #[test]
    fn bounds_utf8_text_without_splitting_characters() {
        let input = "a".repeat(20) + "\u{1F600}";
        let output = redact_and_bound_text(&input, 16);

        assert!(output.len() <= 16);
        assert!(output.ends_with(TRUNCATED));
    }

    #[test]
    fn bounded_json_stays_valid_and_redacted() {
        let input = serde_json::json!({
            "apiKey": "must-not-survive",
            "payload": "x".repeat(1024),
        })
        .to_string();

        let output = redact_and_bound_json_text(&input, 256);

        assert!(output.len() <= 256);
        assert!(!output.contains("must-not-survive"));
        let parsed: Value = serde_json::from_str(&output).expect("bounded json remains valid");
        assert_eq!(parsed["$openCoworkTruncated"], true);
    }

    #[test]
    fn diagnostic_labels_reject_free_form_content() {
        assert_eq!(
            diagnostic_label("scheduler.task_completed"),
            "scheduler.task_completed"
        );
        assert_eq!(diagnostic_label("arbitrary secret sentence"), REDACTED);
    }
}
