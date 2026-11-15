//! Bounded terminal text sanitization for explicit AI snapshots.
//!
//! This module is never connected to live PTY forwarding. It runs only after
//! Control Mode schedules a quiet-period capture for optional classification.

#[derive(Debug, Default, Clone, Copy)]
enum SanitizerState {
    #[default]
    Ground,
    Escape,
    Csi,
    Osc,
    OscEscape,
}

#[derive(Debug, Default)]
struct StreamingTerminalSanitizer {
    state: SanitizerState,
    last_was_carriage_return: bool,
}

impl StreamingTerminalSanitizer {
    fn sanitize(&mut self, raw: &[u8]) -> Vec<u8> {
        let mut result = Vec::with_capacity(raw.len());
        for &byte in raw {
            if matches!(self.state, SanitizerState::Ground) {
                if byte == b'\n' && self.last_was_carriage_return {
                    self.last_was_carriage_return = false;
                    continue;
                }
                self.last_was_carriage_return = byte == b'\r';
            }
            match self.state {
                SanitizerState::Ground => match byte {
                    0x1b => self.state = SanitizerState::Escape,
                    b'\r' | b'\n' => result.push(b'\n'),
                    b'\t' | 0x20..=0x7e | 0x80..=0xff => result.push(byte),
                    _ => {}
                },
                SanitizerState::Escape => {
                    self.state = match byte {
                        b'[' => SanitizerState::Csi,
                        b']' => SanitizerState::Osc,
                        _ => SanitizerState::Ground,
                    };
                }
                SanitizerState::Csi => {
                    if (0x40..=0x7e).contains(&byte) {
                        self.state = SanitizerState::Ground;
                    }
                }
                SanitizerState::Osc => match byte {
                    0x07 => self.state = SanitizerState::Ground,
                    0x1b => self.state = SanitizerState::OscEscape,
                    _ => {}
                },
                SanitizerState::OscEscape => {
                    self.state = if byte == b'\\' {
                        SanitizerState::Ground
                    } else if byte == 0x1b {
                        SanitizerState::OscEscape
                    } else {
                        SanitizerState::Osc
                    };
                }
            }
        }
        result
    }
}

pub(crate) fn sanitize_terminal_output(raw: &[u8]) -> String {
    let mut sanitizer = StreamingTerminalSanitizer::default();
    String::from_utf8_lossy(&sanitizer.sanitize(raw)).into_owned()
}


