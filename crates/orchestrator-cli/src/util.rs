use std::time::{Duration, SystemTime, UNIX_EPOCH};
use time::{format_description::FormatItem, macros::format_description, OffsetDateTime};

pub const RUN_ID_TIME_FORMAT: &[FormatItem<'_>] =
    format_description!("[year][month][day]T[hour][minute][second]Z");

pub fn batch_id(started_at: OffsetDateTime) -> Result<String, String> {
    started_at
        .format(RUN_ID_TIME_FORMAT)
        .map_err(|error| format!("failed to format batch timestamp: {error}"))
}

pub fn run_id(batch_id: &str, harness: &str, model: &str, test: &str) -> String {
    let harness = sanitize_fragment(harness);
    let model = sanitize_fragment(model);
    let test = sanitize_fragment(test);
    let suffix = short_suffix();
    format!("{batch_id}_{harness}_{model}_{test}_{suffix}")
}

pub fn sanitize_fragment(value: &str) -> String {
    let mut sanitized = String::new();
    let mut last_was_dash = false;

    for character in value.chars() {
        let next = if character.is_ascii_alphanumeric() {
            last_was_dash = false;
            Some(character.to_ascii_lowercase())
        } else if !last_was_dash {
            last_was_dash = true;
            Some('-')
        } else {
            None
        };

        if let Some(character) = next {
            sanitized.push(character);
        }
    }

    sanitized.trim_matches('-').to_owned()
}

fn short_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{:06x}", nanos & 0x00ff_ffff)
}

pub fn format_timestamp(timestamp: OffsetDateTime) -> Result<String, String> {
    timestamp
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|error| format!("failed to format timestamp: {error}"))
}

pub fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}
