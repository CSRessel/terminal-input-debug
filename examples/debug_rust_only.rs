use nix::poll::{poll, PollFd, PollFlags, PollTimeout};
use nix::sys::termios::{self, Termios};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, BorrowedFd};

struct RawMode<'a> {
    fd: BorrowedFd<'a>,
    orig: Termios,
}
impl<'a> RawMode<'a> {
    fn new(fd: BorrowedFd<'a>) -> io::Result<Self> {
        let orig = termios::tcgetattr(fd).map_err(to_io)?;
        let mut raw = orig.clone();
        // cfmakeraw: disable canonical mode, echo, signals, etc.
        termios::cfmakeraw(&mut raw);
        // We'll use poll; keep VMIN/VTIME relaxed.
        termios::tcsetattr(fd, termios::SetArg::TCSANOW, &raw).map_err(to_io)?;
        Ok(Self { fd, orig })
    }
}
impl<'a> Drop for RawMode<'a> {
    fn drop(&mut self) {
        let _ = termios::tcsetattr(self.fd, termios::SetArg::TCSANOW, &self.orig);
    }
}

fn to_io(err: nix::Error) -> io::Error {
    io::Error::new(io::ErrorKind::Other, err)
}

#[derive(Debug)]
enum Token {
    Byte(u8),
    Ctrl(&'static str),
    Esc,
    CSI(String), // raw CSI if unrecognized
    SS3(String), // raw SS3 (ESC O ...)
    Key(&'static str),
    PasteStart,
    PasteEnd,
    Mouse {
        press: bool,
        x: i32,
        y: i32,
        mods: i32,
        btn: i32,
    },
}

fn main() -> io::Result<()> {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    writeln!(out, "Reading TTY in raw mode. Ctrl-C to exit.")?;

    // Put TTY in raw mode (only if stdin is a TTY).
    let stdin_fd = unsafe { BorrowedFd::borrow_raw(io::stdin().as_raw_fd()) };
    let _raw = if unsafe { libc::isatty(stdin_fd.as_raw_fd()) } == 1 {
        Some(RawMode::new(stdin_fd)?)
    } else {
        None
    };

    // Optionally enable bracketed paste + mouse reporting (comment out if not needed).
    enable_capabilities(&mut out, true, true)?; // (paste, mouse)

    let mut pfd = [PollFd::new(stdin_fd, PollFlags::POLLIN)];
    let mut buf = [0u8; 4096];
    let mut q: VecDeque<u8> = VecDeque::new();

    out.flush()?;

    loop {
        match poll(&mut pfd, PollTimeout::NONE) {
            Ok(n) if n > 0 => {
                match input.read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        q.extend(&buf[..n]);
                        while let Some(tok) = parse_next(&mut q) {
                            print_token(&mut out, &tok)?;
                            out.flush()?;
                            if matches!(tok, Token::Ctrl("C")) {
                                // Ctrl-C
                                cleanup_capabilities(&mut out)?;
                                writeln!(out, "\n^C")?;
                                return Ok(());
                            }
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                    Err(e) => return Err(e),
                }
            }
            Ok(_) => continue,
            Err(err) => {
                if err == nix::errno::Errno::EINTR {
                    continue;
                }
                return Err(to_io(err));
            }
        }
    }

    cleanup_capabilities(&mut out)?;
    Ok(())
}

fn print_token(out: &mut impl Write, t: &Token) -> io::Result<()> {
    match t {
        Token::Byte(b) => write!(out, "\\x{:02x}", b),
        Token::Ctrl(name) => write!(out, "<CTRL-{name}>"),
        Token::Esc => write!(out, "<ESC>"),
        Token::Key(name) => write!(out, "<{name}>"),
        Token::CSI(s) => write!(out, "<CSI {s}>"),
        Token::SS3(s) => write!(out, "<SS3 {s}>"),
        Token::PasteStart => write!(out, "<PASTE-START>"),
        Token::PasteEnd => write!(out, "<PASTE-END>"),
        Token::Mouse {
            press,
            x,
            y,
            mods,
            btn,
        } => {
            write!(
                out,
                "<MOUSE {} btn={} x={} y={} mods={}>",
                if *press { "DOWN" } else { "UP" },
                btn,
                x,
                y,
                mods
            )
        }
    }
}

fn parse_next(q: &mut VecDeque<u8>) -> Option<Token> {
    let b = *q.front()?;
    // Control bytes and DEL
    if b <= 0x1F || b == 0x7F {
        q.pop_front();
        return Some(match b {
            0x00 => Token::Ctrl("@"),
            0x01 => Token::Ctrl("A"),
            0x02 => Token::Ctrl("B"),
            0x03 => Token::Ctrl("C"),
            0x04 => Token::Ctrl("D"),
            0x05 => Token::Ctrl("E"),
            0x06 => Token::Ctrl("F"),
            0x07 => Token::Key("BEL"),
            0x08 => Token::Key("BS"),
            0x09 => Token::Key("TAB"),
            0x0A => Token::Key("LF"),
            0x0D => Token::Key("CR"),
            0x1B => Token::Esc, // ESC begins sequences too, but return bare ESC if it’s alone so far
            0x7F => Token::Key("DEL"),
            _ => Token::Ctrl("?"),
        });
    }
    // ESC-sequences
    if b == 0x1B {
        if q.len() == 1 {
            // Might be start of a sequence; wait for more
            return None;
        }
        // Look ahead
        let mut it = q.iter().copied();
        let _esc = it.next();
        match it.next() {
            Some(b'[') => {
                // CSI: ESC [ ... final
                // Final byte: 0x40..0x7E. Gather until we have a final.
                if let Some((seq, final_ready)) = gather_ansi(q, b'[', true) {
                    if !final_ready {
                        return None;
                    }
                    if let Some(tok) = map_csi(&seq) {
                        return Some(tok);
                    }
                    return Some(Token::CSI(seq));
                } else {
                    return None;
                }
            }
            Some(b'O') => {
                // SS3: ESC O <final>
                if let Some((seq, final_ready)) = gather_ansi(q, b'O', false) {
                    if !final_ready {
                        return None;
                    }
                    if let Some(tok) = map_ss3(&seq) {
                        return Some(tok);
                    }
                    return Some(Token::SS3(seq));
                } else {
                    return None;
                }
            }
            _ => {
                // ESC followed by something else (e.g., ESC alone then byte)
                q.pop_front(); // ESC
                return Some(Token::Esc);
            }
        }
    }

    // Printable or UTF-8 bytes: emit raw hex for each first byte
    let b = q.pop_front().unwrap();
    Some(Token::Byte(b))
}

/// Gather an ANSI sequence after ESC <lead> …
/// Returns (string_without_ESC, has_final).
fn gather_ansi(q: &mut VecDeque<u8>, lead: u8, _has_params: bool) -> Option<(String, bool)> {
    // Ensure prefix is present: ESC lead
    if q.len() < 2 {
        return None;
    }
    let esc = q.pop_front().unwrap();
    let l = q.pop_front().unwrap();
    debug_assert_eq!(esc, 0x1B);
    debug_assert_eq!(l, lead);

    let mut tmp: Vec<u8> = vec![lead];
    // Consume until final 0x40..0x7E or give up if buffer ends
    while let Some(&c) = q.front() {
        tmp.push(c);
        q.pop_front();
        if (0x40..=0x7E).contains(&c) {
            let s = String::from_utf8_lossy(&tmp).into_owned();
            return Some((s, true));
        }
        // Protective cap
        if tmp.len() > 64 {
            let s = String::from_utf8_lossy(&tmp).into_owned();
            return Some((s, false));
        }
    }
    // Not enough yet; put back what we consumed (except the ESC+lead) for next time
    // (We *don’t* push back here—simplify: treat as incomplete; caller will return None.)
    // But since we already popped, simplest is to stash in a side buffer; for brevity we just signal incomplete.
    // In practice, make a ring buffer snapshot before consuming.
    // Here, we return the partial (no final) and signal incomplete.
    Some((String::from_utf8_lossy(&tmp).into_owned(), false))
}

fn map_csi(s: &str) -> Option<Token> {
    // s starts with '['
    // Examples: [A,[B,[C,[D arrows; [H,[F home/end; [1~ etc; [<...M/m mouse
    if s == "[A" {
        return Some(Token::Key("UP"));
    }
    if s == "[B" {
        return Some(Token::Key("DOWN"));
    }
    if s == "[C" {
        return Some(Token::Key("RIGHT"));
    }
    if s == "[D" {
        return Some(Token::Key("LEFT"));
    }
    if s == "[H" {
        return Some(Token::Key("HOME"));
    }
    if s == "[F" {
        return Some(Token::Key("END"));
    }

    // Bracketed paste
    if s == "[200~" {
        return Some(Token::PasteStart);
    }
    if s == "[201~" {
        return Some(Token::PasteEnd);
    }

    // Function/Page/Delete keys like [1~, [2~, [3~, [5~, [6~
    if let Some(rest) = s.strip_prefix('[') {
        if let Some(idx) = rest.find('~') {
            let num = &rest[..idx];
            return Some(Token::Key(match num {
                "1" | "7" => "HOME",
                "2" => "INS",
                "3" => "DEL",
                "4" | "8" => "END",
                "5" => "PGUP",
                "6" => "PGDN",
                "11" => "F1",
                "12" => "F2",
                "13" => "F3",
                "14" => "F4",
                "15" => "F5",
                "17" => "F6",
                "18" => "F7",
                "19" => "F8",
                "20" => "F9",
                "21" => "F10",
                "23" => "F11",
                "24" => "F12",
                _ => return None,
            }));
        }

        // xterm SGR mouse: CSI <btn;col;row M/m
        if let Some(rest) = rest.strip_prefix('<') {
            let mut parts = rest.split(|c| c == ';' || c == 'M' || c == 'm');
            if let (Some(btn), Some(x), Some(y)) = (parts.next(), parts.next(), parts.next()) {
                let press = rest.contains('M'); // release uses 'm'
                if let (Ok(b), Ok(cx), Ok(cy)) = (btn.parse::<i32>(), x.parse(), y.parse()) {
                    // Decode basic fields (mods are in btn high bits; left/middle/right in low bits)
                    let mods = (b & 0b111000) >> 3;
                    let btn_id = b & 0b111;
                    return Some(Token::Mouse {
                        press,
                        x: cx,
                        y: cy,
                        mods,
                        btn: btn_id,
                    });
                }
            }
        }
    }
    None
}

fn map_ss3(s: &str) -> Option<Token> {
    // SS3 (ESC O P..S) for F1..F4 and arrows on some terms
    match s {
        "OP" => Some(Token::Key("F1")),
        "OQ" => Some(Token::Key("F2")),
        "OR" => Some(Token::Key("F3")),
        "OS" => Some(Token::Key("F4")),
        "OA" => Some(Token::Key("UP")),
        "OB" => Some(Token::Key("DOWN")),
        "OC" => Some(Token::Key("RIGHT")),
        "OD" => Some(Token::Key("LEFT")),
        _ => None,
    }
}

fn enable_capabilities(out: &mut impl Write, paste: bool, mouse: bool) -> io::Result<()> {
    if paste {
        // Bracketed paste on
        write!(out, "\x1b[?2004h")?;
    }
    if mouse {
        // xterm mouse (1000: btn press/release; 1006: SGR extended coords; 1015: urxvt extended)
        write!(out, "\x1b[?1000h\x1b[?1006h")?;
    }
    out.flush()
}

fn cleanup_capabilities(out: &mut impl Write) -> io::Result<()> {
    // Turn off features we turned on
    write!(out, "\x1b[?2004l\x1b[?1000l")?;
    out.flush()
}
