extern crate termios;

use std::os::unix::io::{AsRawFd, RawFd};
use std::io;
use std::io::prelude::*;
use std::io::ErrorKind;
use std::char;

use termios::*;

pub struct Kilo {
    stdin_fd: RawFd,
    orig_termios: Termios,
}

impl Drop for Kilo {
    fn drop(&mut self) {
        if let Err(e) = self.disable_raw_mode() {
            eprintln!("Unable to gracefully exit Kilo: {}", e);
        }
    }
}

impl Kilo {
    pub fn new() -> io::Result<Self> {
        let stdin_fd = io::stdin().as_raw_fd();
        let orig_termios = Termios::from_fd(stdin_fd)?;

        Ok(Kilo {
            stdin_fd,
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

    pub fn run(self) -> io::Result<()> {
        self.enable_raw_mode()?;

        let mut buffer = [0];

        loop {
            if let Err(e) = io::stdin().read(&mut buffer) {
                if e.kind() != ErrorKind::Interrupted {
                    return Err(e);
                }
            }

            let b = buffer[0];
            let c = char::from_u32(b as u32).unwrap();

            if c.is_control() {
                print!("{}\r\n", b);
            } else {
                print!("{} ('{}')\r\n", b, c);
            }

            if c == 'q' {
                break;
            }
        }

        Ok(())
    }
}
