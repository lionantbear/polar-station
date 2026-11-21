//! Filter the Ghostty startup identity replies recognized by both terminal adapters.

use std::time::{Duration, Instant};

const STARTUP_REPLY_FILTER_WINDOW: Duration = Duration::from_secs(5);
const MAX_STARTUP_REPLY_BYTES: usize = 128;

#[derive(Debug, Default)]
pub struct StartupTerminalReplyFilter {
    active_until: Option<Instant>,
    pending: Vec<u8>,
}

impl StartupTerminalReplyFilter {
    pub fn arm(&mut self, now: Instant) {
        self.active_until = Some(now + STARTUP_REPLY_FILTER_WINDOW);
        self.pending.clear();
    }

    pub fn consume(&mut self, input: &[u8], now: Instant, unavailable: bool) -> Vec<u8> {
        // Automatic replies received during recovery are not evidence that a
        // user's keystrokes were lost. Reuse the existing bounded parser;
        // forward focus reports normally once the attachment is Ready.
        let active = unavailable || self.active_until.is_some_and(|deadline| now < deadline);
        if !active {
            self.active_until = None;
            let mut output = std::mem::take(&mut self.pending);
            output.extend_from_slice(input);
            return output;
        }

        self.pending.extend_from_slice(input);
        let mut output = Vec::with_capacity(self.pending.len());
        let mut offset = 0;
        while offset < self.pending.len() {
            if self.pending[offset] != 0x1b {
                output.push(self.pending[offset]);
                offset += 1;
                continue;
            }
            let input = &self.pending[offset..];
            let reply =
                if unavailable && (input.starts_with(b"\x1b[I") || input.starts_with(b"\x1b[O")) {
                    StartupReplyMatch::Complete(3)
                } else {
                    startup_identity_reply(input)
                };
            match reply {
                StartupReplyMatch::Complete(length) => offset += length,
                StartupReplyMatch::Prefix => break,
                StartupReplyMatch::NoMatch => {
                    output.push(self.pending[offset]);
                    offset += 1;
                }
            }
        }
        self.pending.drain(..offset);
        output
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupReplyMatch {
    Complete(usize),
    Prefix,
    NoMatch,
}

fn startup_identity_reply(input: &[u8]) -> StartupReplyMatch {
    const SECONDARY_ATTRIBUTES_PREFIX: &[u8] = b"\x1b[>";
    const GHOSTTY_VERSION_PREFIX: &[u8] = b"\x1bP>|ghostty ";

    if SECONDARY_ATTRIBUTES_PREFIX.starts_with(input) {
        return StartupReplyMatch::Prefix;
    }
    if input.starts_with(SECONDARY_ATTRIBUTES_PREFIX) {
        let mut saw_digit = false;
        for (index, byte) in input[SECONDARY_ATTRIBUTES_PREFIX.len()..]
            .iter()
            .copied()
            .enumerate()
        {
            let length = SECONDARY_ATTRIBUTES_PREFIX.len() + index + 1;
            match byte {
                b'0'..=b'9' => saw_digit = true,
                b';' if saw_digit => {}
                b'c' if saw_digit => return StartupReplyMatch::Complete(length),
                _ => return StartupReplyMatch::NoMatch,
            }
            if length >= MAX_STARTUP_REPLY_BYTES {
                return StartupReplyMatch::NoMatch;
            }
        }
        return StartupReplyMatch::Prefix;
    }

    if GHOSTTY_VERSION_PREFIX.starts_with(input) {
        return StartupReplyMatch::Prefix;
    }
    if input.starts_with(GHOSTTY_VERSION_PREFIX) {
        let payload = &input[GHOSTTY_VERSION_PREFIX.len()..];
        let mut index = 0;
        while index < payload.len() {
            if payload[index] == 0x1b {
                if index + 1 == payload.len() {
                    return StartupReplyMatch::Prefix;
                }
                if payload[index + 1] == b'\\' {
                    return StartupReplyMatch::Complete(GHOSTTY_VERSION_PREFIX.len() + index + 2);
                }
                return StartupReplyMatch::NoMatch;
            }
            if !payload[index].is_ascii_graphic() {
                return StartupReplyMatch::NoMatch;
            }
            index += 1;
            if GHOSTTY_VERSION_PREFIX.len() + index >= MAX_STARTUP_REPLY_BYTES {
                return StartupReplyMatch::NoMatch;
            }
        }
        return StartupReplyMatch::Prefix;
    }

    StartupReplyMatch::NoMatch
}


