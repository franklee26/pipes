use crate::{LogStream, RunLogEntry, registry::runs::now};

const OUTPUT_LIMIT: usize = 64 * 1024 * 1024;
const ENTRY_LIMIT: usize = 8192;

#[derive(Default)]
pub(super) struct OutputBuffer {
    pub entries: Vec<RunLogEntry>,
    bytes: usize,
    truncated: bool,
}

impl OutputBuffer {
    pub fn push(
        &mut self,
        stage: Option<&str>,
        command: Option<&str>,
        stream: LogStream,
        text: &str,
    ) {
        if text.is_empty() || (self.truncated && stream != LogStream::System) {
            return;
        }
        let mut text = text;
        while !text.is_empty() {
            let mut end = text.len().min(ENTRY_LIMIT);
            if stream != LogStream::System {
                end = end.min(OUTPUT_LIMIT.saturating_sub(self.bytes));
            }
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            if end == 0 {
                self.truncated = true;
                self.push(None, None, LogStream::System,
                    "[output limit] The 64 MiB retained-output limit was reached. Further stdout/stderr is drained but not retained.\n");
                return;
            }
            self.entries.push(RunLogEntry {
                sequence: 0,
                recorded_at: now(),
                stage_id: stage.map(str::to_owned),
                command_id: command.map(str::to_owned),
                stream,
                text: text[..end].to_owned(),
            });
            if stream != LogStream::System {
                self.bytes += end;
            }
            text = &text[end..];
        }
    }
}

pub(super) struct TextStream {
    bytes: Vec<u8>,
    pending: String,
    secrets: Vec<String>,
}

impl TextStream {
    pub fn new(mut secrets: Vec<String>) -> Self {
        secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
        secrets.dedup();
        Self {
            bytes: Vec::new(),
            pending: String::new(),
            secrets,
        }
    }

    pub fn push(&mut self, bytes: &[u8], finished: bool) -> String {
        self.bytes.extend_from_slice(bytes);
        loop {
            match std::str::from_utf8(&self.bytes) {
                Ok(text) => {
                    self.pending.push_str(text);
                    self.bytes.clear();
                    break;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    self.pending
                        .push_str(&String::from_utf8_lossy(&self.bytes[..valid]));
                    self.bytes.drain(..valid);
                    if let Some(length) = error.error_len() {
                        self.pending.push('\u{fffd}');
                        self.bytes.drain(..length);
                    } else {
                        if finished {
                            self.pending.push('\u{fffd}');
                            self.bytes.clear();
                        }
                        break;
                    }
                }
            }
        }
        // Retain only a possible secret prefix across chunks, not whole lines.
        // This keeps partial, newline-free progress visible without leaking split credentials.
        let mut output = String::new();
        while !self.pending.is_empty() {
            let match_at = self
                .secrets
                .iter()
                .filter_map(|secret| {
                    self.pending
                        .find(secret)
                        .map(|offset| (offset, secret.len()))
                })
                .min_by_key(|(offset, length)| (*offset, std::cmp::Reverse(*length)));
            if let Some((offset, length)) = match_at {
                output.push_str(&self.pending[..offset]);
                output.push_str("[REDACTED]");
                self.pending.drain(..offset + length);
                continue;
            }
            if finished {
                output.push_str(&self.pending);
                self.pending.clear();
                break;
            }
            let held = self
                .secrets
                .iter()
                .flat_map(|secret| {
                    (1..secret.len().min(self.pending.len() + 1))
                        .rev()
                        .find(|length| {
                            self.pending
                                .as_bytes()
                                .ends_with(&secret.as_bytes()[..*length])
                        })
                })
                .max()
                .unwrap_or(0);
            let end = self.pending.len() - held;
            output.push_str(&self.pending[..end]);
            self.pending.drain(..end);
            break;
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_is_live_without_a_newline_and_unicode_survives_byte_boundaries() {
        let mut stream = TextStream::new(Vec::new());
        assert_eq!(stream.push(b"progress: ", false), "progress: ");
        assert_eq!(stream.push(&[0xe2, 0x9c], false), "");
        assert_eq!(stream.push(&[0x93, b'\r'], false), "\u{2713}\r");
        assert_eq!(stream.push(b"next", true), "next");
    }

    #[test]
    fn credentials_are_redacted_across_every_chunk_boundary() {
        let secret = "sample-secret-value";
        for split in 1..secret.len() {
            let mut stream = TextStream::new(vec![secret.to_owned()]);
            let first = stream.push(&secret.as_bytes()[..split], false);
            let last = stream.push(&secret.as_bytes()[split..], true);
            assert_eq!(format!("{first}{last}"), "[REDACTED]");
        }
        let mut stream = TextStream::new(vec![secret.to_owned()]);
        assert_eq!(
            stream.push(b"unrelated progress", false),
            "unrelated progres"
        );
        assert_eq!(stream.push(&[], true), "s");
    }

    #[test]
    fn malformed_utf8_is_replaced_without_losing_valid_neighbors() {
        let mut stream = TextStream::new(Vec::new());
        assert_eq!(
            stream.push(&[b'a', 0xff, b'b', 0xe2], true),
            "a\u{fffd}b\u{fffd}"
        );
    }

    #[test]
    fn retained_output_is_bounded_and_truncation_is_explicit() {
        let mut output = OutputBuffer {
            bytes: OUTPUT_LIMIT - 3,
            ..OutputBuffer::default()
        };
        output.push(None, None, LogStream::Stdout, "abcdef");
        assert_eq!(output.entries[0].text, "abc");
        assert_eq!(output.entries[1].stream, LogStream::System);
        assert!(output.entries[1].text.contains("64 MiB"));
        let count = output.entries.len();
        output.push(None, None, LogStream::Stdout, "not retained");
        assert_eq!(output.entries.len(), count);
    }
}
