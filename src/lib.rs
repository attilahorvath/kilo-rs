extern crate libc;
extern crate termios;

use std::os::unix::io::{AsRawFd, RawFd};
use std::io;
use std::io::prelude::*;
use std::io::ErrorKind;
use std::char;

use libc::{TIOCGWINSZ, ioctl, winsize};
use termios::*;

const KILO_VERSION: Option<&'static str> = option_env!("CARGO_PKG_VERSION");

fn ctrl_key(k: char) -> u8 {
    (k as u8) & 0x1f
}

pub fn clear_screen() -> io::Result<()> {
    io::stdout().write("\x1b[2J".as_bytes())?;
    io::stdout().write("\x1b[H".as_bytes())?;
    io::stdout().flush()?;

    Ok(())
}

#[derive(PartialEq)]
enum EditorKey {
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    DelKey,
    HomeKey,
    EndKey,
    PageUp,
    PageDown,
    Char(u8),
}

pub struct Kilo {
    stdin_fd: RawFd,
    cx: u16,
    cy: u16,
    screenrows: u16,
    screencols: u16,
    orig_termios: Termios,
}

use EditorKey::*;

impl Drop for Kilo {
    fn drop(&mut self) {
        if let Err(e) = self.disable_raw_mode() {
            eprintln!("Unable to restore canonical mode: {}", e);
        }
    }
}

impl Kilo {
    pub fn new() -> io::Result<Self> {
        let stdin_fd = io::stdin().as_raw_fd();
        let orig_termios = Termios::from_fd(stdin_fd)?;

        Ok(Kilo {
            stdin_fd,
            cx: 0,
            cy: 0,
            screenrows: 0,
            screencols: 0,
            orig_termios,
        })
    }

    fn disable_raw_mode(&self) -> io::Result<()> {
        tcsetattr(self.stdin_fd, TCSAFLUSH, &self.orig_termios)
    }

    fn enable_raw_mode(&self) -> io::Result<()> {
        let mut raw = self.orig_termios.clone();

        raw.c_iflag &= !(BRKINT | ICRNL | INPCK | ISTRIP | IXON);
        raw.c_oflag &= !(OPOST);
        raw.c_cflag |= CS8;
        raw.c_lflag &= !(ECHO | ICANON | IEXTEN | ISIG);
        raw.c_cc[VMIN] = 0;
        raw.c_cc[VTIME] = 1;

        tcsetattr(self.stdin_fd, TCSAFLUSH, &raw)
    }

    fn editor_read_key(&self) -> io::Result<EditorKey> {
        let mut buffer = [0];

        while let Err(e) = io::stdin().read(&mut buffer) {
            if e.kind() != ErrorKind::Interrupted {
                return Err(e);
            }
        }

        let c = buffer[0];

        if c == '\x1b' as u8 {
            let mut seq = [0; 3];

            if io::stdin().read(&mut seq[0..1])? != 1 {
                return Ok(Char(c));
            }

            if io::stdin().read(&mut seq[1..2])? != 1 {
                return Ok(Char(c));
            }

            if seq[0] == '[' as u8 {
                if seq[1] >= '0' as u8 && seq[1] <= '9' as u8 {
                    if io::stdin().read(&mut seq[2..3])? != 1 {
                        return Ok(Char(c));
                    }

                    if seq[2] == '~' as u8 {
                        match seq[1] as char {
                            '1' => return Ok(HomeKey),
                            '3' => return Ok(DelKey),
                            '4' => return Ok(EndKey),
                            '5' => return Ok(PageUp),
                            '6' => return Ok(PageDown),
                            '7' => return Ok(HomeKey),
                            '8' => return Ok(EndKey),
                            _ => return Ok(Char(c)),
                        }
                    }
                } else {
                    match seq[1] as char {
                        'A' => return Ok(ArrowUp),
                        'B' => return Ok(ArrowDown),
                        'C' => return Ok(ArrowRight),
                        'D' => return Ok(ArrowLeft),
                        'H' => return Ok(HomeKey),
                        'F' => return Ok(EndKey),
                        _ => return Ok(Char(c)),
                    }
                }
            } else if seq[0] == 'O' as u8 {
                match seq[1] as char {
                    'H' => return Ok(HomeKey),
                    'F' => return Ok(EndKey),
                    _ => return Ok(Char(c)),
                }
            }

            return Ok(Char(c));
        } else {
            Ok(Char(c))
        }
    }

    fn get_cursor_position(&self) -> io::Result<(u16, u16)> {
        io::stdout().write("\x1b[6n".as_bytes())?;
        io::stdout().flush()?;

        let mut buffer = [0; 32];
        let bytes_read = io::stdin().read(&mut buffer)?;

        let s = std::str::from_utf8(&buffer[..bytes_read]).map_err(|_| {
            io::Error::last_os_error()
        })?;

        if !s.starts_with("\x1b[") || !s.ends_with("R") {
            return Err(io::Error::last_os_error());
        }

        let mut parts = s[2..(s.len() - 1)].split(';').map(
            |i| i.parse().unwrap_or(0),
        );
        let rows = parts.next().unwrap_or(0);
        let cols = parts.next().unwrap_or(0);

        Ok((rows, cols))
    }

    fn get_window_size(&self) -> io::Result<(u16, u16)> {
        unsafe {
            let ws: winsize = std::mem::uninitialized();

            if ioctl(self.stdin_fd, TIOCGWINSZ, &ws) == -1 || ws.ws_col == 0 || ws.ws_row == 0 {
                io::stdout().write("\x1b[999C\x1b[999B".as_bytes())?;
                io::stdout().flush()?;

                self.get_cursor_position()
            } else {
                Ok((ws.ws_row as u16, ws.ws_col as u16))
            }
        }
    }

    fn editor_draw_rows(&self, buffer: &mut String) -> io::Result<()> {
        for y in 0..self.screenrows {
            if y == self.screenrows / 3 {
                let mut welcome = match KILO_VERSION {
                    Some(version) => format!("Kilo editor -- version {}", version),
                    None => "Kilo editor".to_string(),
                };

                welcome.truncate(self.screencols as usize);

                let mut padding = (self.screencols - welcome.len() as u16) / 2;

                if padding > 0 {
                    buffer.push('~');
                    padding -= 1;
                }

                for _ in 0..padding {
                    buffer.push(' ');
                }

                buffer.push_str(&welcome);
            } else {
                buffer.push('~');
            }

            buffer.push_str("\x1b[K");
            if y < self.screenrows - 1 {
                buffer.push_str("\r\n");
            }
        }

        Ok(())
    }

    fn editor_refresh_screen(&self) -> io::Result<()> {
        let mut buffer = String::new();

        buffer.push_str("\x1b[?25l");
        buffer.push_str("\x1b[H");

        self.editor_draw_rows(&mut buffer)?;

        buffer.push_str(&format!("\x1b[{};{}H", self.cy + 1, self.cx + 1));
        buffer.push_str("\x1b[?25h");

        io::stdout().write(buffer.as_bytes())?;
        io::stdout().flush()?;

        Ok(())
    }

    fn editor_move_cursor(&mut self, key: EditorKey) {
        match key {
            ArrowLeft => {
                if self.cx != 0 {
                    self.cx -= 1;
                }
            }
            ArrowRight => {
                if self.cx != self.screencols - 1 {
                    self.cx += 1;
                }
            }
            ArrowUp => {
                if self.cy != 0 {
                    self.cy -= 1;
                }
            }
            ArrowDown => {
                if self.cy != self.screenrows - 1 {
                    self.cy += 1;
                }
            }
            _ => {}
        }
    }

    fn editor_process_keypress(&mut self) -> io::Result<bool> {
        let c = self.editor_read_key()?;

        match c {
            Char(c) if c == ctrl_key('q') => return Ok(false),
            HomeKey => self.cx = 0,
            EndKey => self.cx = self.screencols - 1,
            PageUp | PageDown => {
                for _ in 0..self.screenrows {
                    self.editor_move_cursor(if c == PageUp { ArrowUp } else { ArrowDown });
                }
            }
            ArrowUp | ArrowDown | ArrowLeft | ArrowRight => self.editor_move_cursor(c),
            _ => {}
        }

        Ok(true)
    }

    fn init_editor(&mut self) -> io::Result<()> {
        let (screenrows, screencols) = self.get_window_size()?;

        self.screenrows = screenrows;
        self.screencols = screencols;

        Ok(())
    }

    pub fn run(mut self) -> io::Result<()> {
        self.enable_raw_mode()?;
        self.init_editor()?;

        loop {
            self.editor_refresh_screen()?;
            if !self.editor_process_keypress()? {
                break;
            }
        }

        clear_screen()?;

        Ok(())
    }
}
