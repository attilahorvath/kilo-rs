extern crate libc;
extern crate termios;

use std::char;
use std::fs::File;
use std::io;
use std::io::prelude::*;
use std::io::{BufReader, ErrorKind};
use std::os::unix::io::{AsRawFd, RawFd};
use std::time::{Duration, Instant};

use libc::{TIOCGWINSZ, ioctl, winsize};
use termios::*;

const KILO_VERSION: Option<&'static str> = option_env!("CARGO_PKG_VERSION");
const KILO_TAB_STOP: usize = 8;

#[inline]
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

struct Row {
    chars: String,
    render: String,
}

pub struct Kilo {
    stdin_fd: RawFd,
    cx: usize,
    cy: usize,
    rx: usize,
    rowoff: usize,
    coloff: usize,
    screenrows: usize,
    screencols: usize,
    rows: Vec<Row>,
    filename: String,
    statusmsg: String,
    statusmsg_time: Instant,
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
            rx: 0,
            rowoff: 0,
            coloff: 0,
            screenrows: 0,
            screencols: 0,
            rows: Vec::new(),
            filename: String::new(),
            statusmsg: String::new(),
            statusmsg_time: Instant::now(),
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

    fn get_cursor_position(&self) -> io::Result<(usize, usize)> {
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

    fn get_window_size(&self) -> io::Result<(usize, usize)> {
        unsafe {
            let ws: winsize = std::mem::uninitialized();

            if ioctl(self.stdin_fd, TIOCGWINSZ, &ws) == -1 || ws.ws_col == 0 || ws.ws_row == 0 {
                io::stdout().write("\x1b[999C\x1b[999B".as_bytes())?;
                io::stdout().flush()?;

                self.get_cursor_position()
            } else {
                Ok((ws.ws_row as usize, ws.ws_col as usize))
            }
        }
    }

    fn editor_row_cx_to_rx(&self, row: &Row, cx: usize) -> usize {
        let mut rx = 0;

        for j in 0..cx {
            if let Some('\t') = row.chars.chars().nth(j) {
                rx += (KILO_TAB_STOP - 1) - (rx % KILO_TAB_STOP);
            }
            rx += 1;
        }

        rx
    }

    fn editor_update_row(&self, row: &mut Row) {
        let spaces = (0..KILO_TAB_STOP).map(|_| ' ').collect::<String>();
        row.render = row.chars.replace('\t', &spaces);
    }

    fn editor_append_row(&mut self, s: &str) {
        let mut row = Row {
            chars: s.to_string(),
            render: String::new(),
        };

        self.editor_update_row(&mut row);
        self.rows.push(row);
    }

    fn editor_open(&mut self, filename: &str) -> io::Result<()> {
        self.filename = filename.to_string();

        let file = File::open(filename)?;
        let reader = BufReader::new(file);

        for line in reader.lines() {
            self.editor_append_row(&line?);
        }

        Ok(())
    }

    fn editor_scroll(&mut self) {
        self.rx = 0;

        if self.cy < self.rows.len() {
            self.rx = self.editor_row_cx_to_rx(&self.rows[self.cy], self.cx);
        }

        if self.cy < self.rowoff {
            self.rowoff = self.cy;
        }

        if self.cy >= self.rowoff + self.screenrows {
            self.rowoff = self.cy - self.screenrows + 1;
        }

        if self.rx < self.coloff {
            self.coloff = self.rx;
        }

        if self.rx >= self.coloff + self.screencols {
            self.coloff = self.rx - self.screencols + 1;
        }
    }

    fn editor_draw_rows(&self, buffer: &mut String) {
        for y in 0..self.screenrows {
            let filerow = y + self.rowoff;
            if filerow >= self.rows.len() {
                if self.rows.is_empty() && y == self.screenrows / 3 {
                    let mut welcome = match KILO_VERSION {
                        Some(version) => format!("Kilo editor -- version {}", version),
                        None => "Kilo editor".to_string(),
                    };

                    welcome.truncate(self.screencols);

                    let mut padding = (self.screencols - welcome.len()) / 2;

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
            } else {
                let line = &self.rows[filerow].render;
                let mut len = line.len().saturating_sub(self.coloff);
                if len > self.screencols {
                    len = self.screencols;
                }
                if len > 0 {
                    buffer.push_str(&line[(self.coloff)..(self.coloff + len)]);
                }
            }

            buffer.push_str("\x1b[K");
            buffer.push_str("\r\n");
        }
    }

    fn editor_draw_status_bar(&self, buffer: &mut String) {
        buffer.push_str("\x1b[7m");
        let mut status = format!("{:.20} - {} lines", self.filename, self.rows.len());
        status.truncate(self.screencols);
        let rstatus = format!("{}/{}", self.cy + 1, self.rows.len());
        let mut len = status.len();
        buffer.push_str(&status);
        while len < self.screencols {
            if self.screencols - len == rstatus.len() {
                buffer.push_str(&rstatus);
                break;
            } else {
                buffer.push_str(" ");
                len += 1;
            }
        }
        buffer.push_str("\x1b[m");
        buffer.push_str("\r\n");
    }

    fn editor_draw_message_bar(&mut self, buffer: &mut String) {
        buffer.push_str("\x1b[K");
        self.statusmsg.truncate(self.screencols);
        if self.statusmsg_time.elapsed() < Duration::from_secs(5) {
            buffer.push_str(&self.statusmsg);
        }
    }

    fn editor_refresh_screen(&mut self) -> io::Result<()> {
        self.editor_scroll();

        let mut buffer = String::new();

        buffer.push_str("\x1b[?25l");
        buffer.push_str("\x1b[H");

        self.editor_draw_rows(&mut buffer);
        self.editor_draw_status_bar(&mut buffer);
        self.editor_draw_message_bar(&mut buffer);

        buffer.push_str(&format!(
            "\x1b[{};{}H",
            (self.cy - self.rowoff) + 1,
            (self.rx - self.coloff) + 1
        ));
        buffer.push_str("\x1b[?25h");

        io::stdout().write(buffer.as_bytes())?;
        io::stdout().flush()?;

        Ok(())
    }

    fn editor_set_status_message(&mut self, message: &str) {
        self.statusmsg = message.to_string();
        self.statusmsg_time = Instant::now();
    }

    fn editor_move_cursor(&mut self, key: EditorKey) {
        let row = self.rows.get(self.cy);

        match key {
            ArrowLeft => {
                if self.cx != 0 {
                    self.cx -= 1;
                } else if self.cy > 0 {
                    self.cy -= 1;
                    self.cx = self.rows[self.cy].chars.len();
                }
            }
            ArrowRight => {
                if let Some(r) = row {
                    if self.cx < r.chars.len() {
                        self.cx += 1;
                    } else if self.cx == r.chars.len() {
                        self.cy += 1;
                        self.cx = 0;
                    }
                }
            }
            ArrowUp => {
                if self.cy != 0 {
                    self.cy -= 1;
                }
            }
            ArrowDown => {
                if self.cy < self.rows.len() {
                    self.cy += 1;
                }
            }
            _ => {}
        }

        let row = self.rows.get(self.cy);
        let rowlen = if let Some(r) = row { r.chars.len() } else { 0 };

        if self.cx > rowlen {
            self.cx = rowlen;
        }
    }

    fn editor_process_keypress(&mut self) -> io::Result<bool> {
        let c = self.editor_read_key()?;

        match c {
            Char(c) if c == ctrl_key('q') => return Ok(false),
            HomeKey => self.cx = 0,
            EndKey => {
                if self.cy < self.rows.len() {
                    self.cx = self.rows[self.cy].chars.len();
                }
            }
            PageUp | PageDown => {
                if c == PageUp {
                    self.cy = self.rowoff;
                } else if c == PageDown {
                    self.cy = self.rowoff + self.screenrows - 1;
                    if self.cy > self.rows.len() {
                        self.cy = self.rows.len();
                    }
                }
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

        self.screenrows = screenrows - 2;
        self.screencols = screencols;

        Ok(())
    }

    pub fn run(mut self) -> io::Result<()> {
        self.enable_raw_mode()?;
        self.init_editor()?;

        let mut argv = std::env::args();
        argv.next();

        if let Some(filename) = argv.next() {
            self.editor_open(&filename)?;
        }

        self.editor_set_status_message("HELP: Ctrl-Q = quit");

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
