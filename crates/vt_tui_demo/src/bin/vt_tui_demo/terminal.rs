//! The UI in a real terminal: raw mode, the terminal's size and input, and
//! handing the terminal back on every way out, panics included.

use std::{
    io::Write as _,
    sync::{
        Mutex,
        mpsc::{Receiver, Sender},
    },
    time::Instant,
};

use vt_str::Str;
use vt_tui::Options;
use vt_tui_demo::{Player, Scenario};

use crate::cli::{Message, TICK};

/// What puts the terminal back if the program stops before the UI is done.
/// Kept up to date with what's been written, for the panic hook.
static RESTORE: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// Plays the run in the terminal and returns the exit code.
pub fn run(
    scenario: Scenario,
    name: Str,
    agent: bool,
    sender: Sender<Message>,
    receiver: &Receiver<Message>,
) -> u8 {
    #[cfg(windows)]
    {
        // Lets the console interpret escape sequences.
        let _ = crossterm::ansi_support::supports_ansi();
    }
    let size = size().unwrap_or((80, 24));
    let mut options = Options::new(size.0, size.1, name);
    if agent {
        options = options.for_agent();
    }
    let mut player = Player::new(scenario, options);

    if let Err(error) = crossterm::terminal::enable_raw_mode() {
        let _ = writeln!(std::io::stderr(), "error: can't put the terminal in raw mode: {error}");
        return 1;
    }
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        previous_hook(info);
        std::process::exit(101);
    }));
    read_input(sender);

    let code = play(&mut player, size, receiver);
    restore();
    code
}

fn play(player: &mut Player, mut size: (u16, u16), receiver: &Receiver<Message>) -> u8 {
    let start = Instant::now();
    let mut stdout = std::io::stdout();
    let mut stopped = false;
    loop {
        let bytes = player.frame(start.elapsed());
        if !bytes.is_empty() {
            let written = stdout.write_all(&bytes).and_then(|()| stdout.flush());
            if let Ok(mut restore) = RESTORE.lock() {
                *restore = player.restore_sequence();
            }
            if written.is_err() {
                return 1;
            }
        }
        if player.is_done() {
            return player.exit_code();
        }
        if stopped {
            // The run is over, but the UI waits for the user to leave the
            // task view. A signal doesn't wait: the caller restores the
            // terminal.
            return 130;
        }

        let mut message = receiver.recv_timeout(TICK).ok();
        while let Some(received) = message {
            let now = start.elapsed();
            match received {
                #[cfg(not(windows))]
                Message::Input(data) => player.input(&data, now),
                #[cfg(windows)]
                Message::Event(event) => player.input_event(event, now),
                Message::Stop => {
                    player.cancel(now);
                    stopped = true;
                }
            }
            message = receiver.try_recv().ok();
        }
        // Windows reports resizes as console events instead.
        if cfg!(not(windows))
            && let Some(new_size) = self::size()
            && new_size != size
        {
            size = new_size;
            player.resize(size.0, size.1);
        }
    }
}

/// The terminal's size, unless it doesn't report one.
fn size() -> Option<(u16, u16)> {
    crossterm::terminal::size().ok().filter(|&(cols, rows)| cols > 0 && rows > 0)
}

/// Writes what puts the terminal back, and leaves raw mode.
fn restore() {
    let bytes = RESTORE.lock().map(|mut restore| std::mem::take(&mut *restore)).unwrap_or_default();
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(&bytes);
    let _ = stdout.flush();
    let _ = crossterm::terminal::disable_raw_mode();
}

/// Reads the terminal's input on a thread of its own: bytes, as the UI parses
/// them.
#[cfg(not(windows))]
fn read_input(sender: Sender<Message>) {
    use std::io::Read as _;

    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buffer = [0; 4096];
        loop {
            match stdin.read(&mut buffer) {
                Ok(0) => return,
                Ok(read) => {
                    if sender.send(Message::Input(buffer[..read].to_vec())).is_err() {
                        return;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return,
            }
        }
    });
}

/// Reads the console's input on a thread of its own: events, since the
/// console doesn't send escape sequences for keys.
#[cfg(windows)]
fn read_input(sender: Sender<Message>) {
    std::thread::spawn(move || {
        while let Ok(event) = crossterm::event::read() {
            if let Ok(event) = terminput_crossterm::to_terminput(event)
                && sender.send(Message::Event(event)).is_err()
            {
                return;
            }
        }
    });
}
