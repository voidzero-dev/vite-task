//! Parsing the bytes the terminal sends into key, mouse and paste events.

use terminput::Event;

/// Collects bytes until they form an event. An escape on its own is only a
/// key press if no more bytes came with it; otherwise it starts a sequence.
#[derive(Debug, Default)]
pub struct InputParser {
    buffer: Vec<u8>,
}

impl InputParser {
    pub fn feed(&mut self, bytes: &[u8], mut on_event: impl FnMut(Event)) {
        for (index, &byte) in bytes.iter().enumerate() {
            self.buffer.push(byte);
            let more = index + 1 < bytes.len();
            if more && self.buffer.as_slice() == b"\x1b" {
                continue;
            }
            match Event::parse_from(&self.buffer) {
                Ok(Some(event)) => {
                    self.buffer.clear();
                    on_event(event);
                }
                Ok(None) => {}
                Err(_) => self.buffer.clear(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use terminput::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};

    use super::*;

    fn parse(bytes: &[u8]) -> Vec<Event> {
        let mut parser = InputParser::default();
        let mut events = Vec::new();
        parser.feed(bytes, |event| events.push(event));
        events
    }

    #[test]
    fn escape_alone_is_a_key() {
        assert_eq!(parse(b"\x1b"), [Event::Key(KeyEvent::new(KeyCode::Esc))]);
    }

    #[test]
    fn arrow_keys_and_text() {
        assert_eq!(
            parse(b"\x1b[Ah\x03"),
            [
                Event::Key(KeyEvent::new(KeyCode::Up)),
                Event::Key(KeyEvent::new(KeyCode::Char('h'))),
                Event::Key(KeyEvent::new(KeyCode::Char('c')).modifiers(KeyModifiers::CTRL)),
            ]
        );
    }

    #[test]
    fn sgr_mouse() {
        let events = parse(b"\x1b[<0;5;3M");
        let [Event::Mouse(mouse)] = events.as_slice() else { panic!("{events:?}") };
        assert_eq!(mouse.kind, MouseEventKind::Down(MouseButton::Left));
        assert_eq!((mouse.column, mouse.row), (4, 2));
    }
}
