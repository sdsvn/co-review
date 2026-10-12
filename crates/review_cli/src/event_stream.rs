//! Server-sent events (`text/event-stream`), read as they arrive.

use std::io::{self, BufRead};

/// The data of each `message` event of a stream, in order, until it ends or fails.
pub struct Events<R> {
    reader: R,
    line: String,
}

impl<R: BufRead> Events<R> {
    pub fn new(reader: R) -> Self {
        Events {
            reader,
            line: String::new(),
        }
    }
}

impl<R: BufRead> Iterator for Events<R> {
    type Item = io::Result<String>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut data: Option<String> = None;
        let mut event_type = String::new();
        loop {
            self.line.clear();
            match self.reader.read_line(&mut self.line) {
                Err(error) => return Some(Err(error)),
                // The end of the stream also ends a last event without its blank line.
                Ok(0) => return data.filter(|_| is_message(&event_type)).map(Ok),
                Ok(_) => {}
            }
            let line = self.line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                if let Some(data) = data.take()
                    && is_message(&event_type)
                {
                    return Some(Ok(data));
                }
                event_type.clear();
                continue;
            }
            if line.starts_with(':') {
                continue;
            }
            let (field, value) = match line.split_once(':') {
                Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
                None => (line, ""),
            };
            match field {
                "data" => match data.as_mut() {
                    Some(data) => {
                        data.push('\n');
                        data.push_str(value);
                    }
                    None => data = Some(value.to_string()),
                },
                "event" => event_type = value.to_string(),
                _ => {}
            }
        }
    }
}

fn is_message(event_type: &str) -> bool {
    event_type.is_empty() || event_type == "message"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(text: &str) -> Vec<String> {
        Events::new(text.as_bytes())
            .collect::<io::Result<Vec<_>>>()
            .expect("events")
    }

    #[test]
    fn reads_message_events() {
        assert_eq!(
            events(
                "id: 1\nevent: message\ndata: {\"a\":1}\n\n: comment\ndata:{\"b\":\ndata: 2}\r\n\r\nevent: ping\ndata: x\n\ndata: last"
            ),
            vec!["{\"a\":1}", "{\"b\":\n2}", "last"]
        );
    }
}
