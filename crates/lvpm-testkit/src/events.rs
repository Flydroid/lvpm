//! A sink that keeps core's events, and the check that an event is data.

use lvpm_core::events::{Event, EventSink};
use serde_json::Value;
use std::sync::Mutex;

/// Keeps every event it is given, in order.
#[derive(Default)]
pub struct RecordingSink(Mutex<Vec<Event>>);

impl RecordingSink {
    pub fn events(&self) -> Vec<Event> {
        self.0.lock().expect("the recorded events").clone()
    }
}

impl EventSink for RecordingSink {
    fn emit(&self, event: Event) {
        self.0.lock().expect("the recorded events").push(event);
    }
}

/// The fields an event's JSON may hold as text, by the kind of thing they
/// are: no sentence, only data.
const URL_FIELD: &str = "url";
const IDENTITY_FIELDS: &[&str] = &["name", "version", "path"];
/// Enumerated values: the variant itself and the outcome of a feed check.
const ENUMERATED_FIELDS: &[&str] = &["event", "outcome"];

/// The text fields of an event's JSON that are not a package name, version,
/// URL, path or enumerated value, as `field = "text"`.
pub fn fields_that_are_not_data(event: &Event) -> Vec<String> {
    let json = serde_json::to_value(event).expect("an event serialises");
    // `{"event": variant, "data": {fields}}`; a variant without fields has no data.
    let name = json["event"].as_str().expect("an event has a name");
    let variant = ("event".to_string(), Value::from(name));
    let fields = match json.get("data") {
        None => Default::default(),
        Some(data) => data
            .as_object()
            .cloned()
            .expect("an event's data is an object"),
    };
    std::iter::once(variant)
        .chain(fields)
        .filter_map(|(field, value)| match value {
            Value::String(text) if !is_data(&field, &text) => Some(format!("{field} = {text:?}")),
            _ => None,
        })
        .collect()
}

fn is_data(field: &str, text: &str) -> bool {
    match field {
        URL_FIELD => text.starts_with("http://") || text.starts_with("https://"),
        _ if IDENTITY_FIELDS.contains(&field) => !text.is_empty() && !text.contains('\n'),
        _ if ENUMERATED_FIELDS.contains(&field) => is_identifier(text),
        _ => false,
    }
}

fn is_identifier(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use lvpm_core::events::FeedOutcome;
    use std::time::Duration;

    #[test]
    fn a_feed_event_is_data() {
        let checked = Event::FeedChecked {
            url: "http://127.0.0.1:1/feed/index.vipr".into(),
            outcome: FeedOutcome::Downloaded,
            bytes: 10,
            elapsed: Duration::from_millis(5),
        };
        assert_eq!(fields_that_are_not_data(&checked), Vec::<String>::new());
    }

    #[test]
    fn a_sentence_in_an_error_field_is_not_data() {
        let ignored = Event::LocalPackageIgnored {
            path: "pkgs/broken.vip".into(),
            error: "no `spec` member".into(),
        };
        assert_eq!(
            fields_that_are_not_data(&ignored),
            vec!["error = \"no `spec` member\"".to_string()]
        );
    }

    #[test]
    fn a_url_field_must_be_a_url() {
        let start = Event::FeedCheckStart {
            url: "checking feed".into(),
            first_run: true,
        };
        assert_eq!(
            fields_that_are_not_data(&start),
            vec!["url = \"checking feed\"".to_string()]
        );
    }
}
