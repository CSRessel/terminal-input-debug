use clap::Parser;
use color_eyre::Result;
use crossterm::event::{KeyCode, KeyModifiers};
#[cfg(not(unix))]
use eyre::eyre;
use eyre::WrapErr;
#[cfg(unix)]
use libc;
#[cfg(unix)]
use nix::errno::Errno;
#[cfg(unix)]
use nix::poll::{poll, PollFd, PollFlags, PollTimeout};
use ratatui::{
    backend::CrosstermBackend,
    layout::Constraint,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Padding, Row, Table},
    Terminal,
};
#[cfg(unix)]
use std::collections::VecDeque;
use std::io::{self, ErrorKind, Read, Write};
#[cfg(unix)]
use std::os::fd::{AsFd, AsRawFd};
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;
use tracing_appender::rolling;
use tracing_subscriber::{self, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Timeout in seconds before exiting
    #[arg(short, long, default_value_t = 30)]
    timeout: u64,

    /// Maximum number of inputs before exiting
    #[arg(short, long, default_value_t = 10)]
    max_inputs: usize,
}

#[derive(Debug, Clone)]
struct InputEventInfo {
    hex_string: String,
    escaped_string: String,
    guess: GuessInfo,
}

#[derive(Debug, Clone)]
struct GuessInfo {
    key: String,
    code: String,
    modifiers: String,
    kind: String,
    description: String,
}

fn main() -> Result<()> {
    // Install color-eyre for enhanced error reporting
    color_eyre::install().expect("Failed to install color-eyre");

    // Initialize logger
    let _logger_guard = init_logger().expect("Failed to initialize logger");

    tracing::info!("Debug keys application starting");

    let args = Args::parse();

    let result = run(args);

    if let Err(ref e) = result {
        tracing::error!("Application error: {}", e);
    }

    tracing::info!("Debug keys application shutting down");
    result
}

#[cfg(unix)]
fn run(args: Args) -> Result<()> {
    const DRAW_TIMEOUT: Duration = Duration::from_millis(100);
    const FLUSH_TIMEOUT: Duration = Duration::from_millis(35);

    let height = args.max_inputs as u16 + 2; // +2 for header and for event info
    let mut terminal = init_terminal(true, height)?;

    let mut events: Vec<InputEventInfo> = Vec::new();
    let mut input_count = 0usize;

    let timeout_duration = Duration::from_secs(args.timeout);
    let start_time = Instant::now();

    let mut reader = RawInputReader::new(FLUSH_TIMEOUT)?;

    loop {
        if start_time.elapsed() >= timeout_duration {
            break;
        }

        if input_count >= args.max_inputs {
            break;
        }

        if let Some(bytes) = reader.poll_next(DRAW_TIMEOUT)? {
            process_event_bytes(bytes, &mut events, &mut input_count);

            while let Some(extra) = reader.poll_next(Duration::ZERO)? {
                process_event_bytes(extra, &mut events, &mut input_count);
                if input_count >= args.max_inputs {
                    break;
                }
            }
        }

        terminal.draw(|f| {
            let size = f.area();
            let status_text = format!(
                "Inputs: {}/{} | Time: {:.1}s / {}s",
                input_count,
                args.max_inputs,
                start_time.elapsed().as_secs_f32(),
                args.timeout
            );
            let title_line = Line::from(vec![
                Span::from("Events ").style(Style::default().fg(Color::Yellow)),
                Span::from("("),
                Span::from(status_text).style(Style::default().fg(Color::Cyan)),
                Span::from(")"),
            ]);

            let block = Block::default()
                .title(title_line)
                .borders(Borders::NONE)
                .padding(Padding::uniform(0));

            let inner_area = block.inner(size);
            f.render_widget(block, size);

            let header = Row::new(vec![
                Cell::from("Hex"),
                Cell::from("Esc"),
                Cell::from("Key"),
                Cell::from("Code"),
                Cell::from("Mods"),
                Cell::from("Kind"),
                Cell::from("Info"),
            ])
            .style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            );

            let widths = [
                Constraint::Length(18),
                Constraint::Length(20),
                Constraint::Length(12),
                Constraint::Length(14),
                Constraint::Length(12),
                Constraint::Length(12),
                Constraint::Min(10),
            ];

            let events_rows: Vec<Row> = events
                .iter()
                .rev()
                .take(50)
                .map(|info| format_event_info(info))
                .collect();

            let events_table = Table::new(events_rows, widths)
                .header(header)
                .column_spacing(1);

            f.render_widget(events_table, inner_area);
        })?;
    }

    restore_terminal(true, height)?;

    terminal.insert_before(height, |f| {
        use ratatui::widgets::Widget;

        let size = f.area();
        let status_text = format!(
            "Final Inputs: {}/{} | Time: {:.1}s / {}s",
            input_count,
            args.max_inputs,
            start_time.elapsed().as_secs_f32(),
            args.timeout
        );
        let title_line = Line::from(vec![
            Span::from("Final Events ").style(Style::default().fg(Color::Yellow)),
            Span::from("("),
            Span::from(status_text).style(Style::default().fg(Color::Cyan)),
            Span::from(")"),
        ]);

        let block = Block::default()
            .title(title_line)
            .borders(Borders::NONE)
            .padding(Padding::uniform(0));

        let inner_area = block.inner(size.clone());
        block.render(size.clone(), f);

        let header = Row::new(vec![
            Cell::from("Hex"),
            Cell::from("Esc"),
            Cell::from("Key"),
            Cell::from("Code"),
            Cell::from("Mods"),
            Cell::from("Kind"),
            Cell::from("Info"),
        ])
        .style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );

        let widths = [
            Constraint::Length(18),
            Constraint::Length(20),
            Constraint::Length(12),
            Constraint::Length(14),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Min(10),
        ];

        let events_rows: Vec<Row> = events.iter().map(|info| format_event_info(info)).collect();

        let events_table = Table::new(events_rows, widths)
            .header(header)
            .column_spacing(1);

        events_table.render(inner_area, f);
    })?;

    Ok(())
}

#[cfg(not(unix))]
fn run(_args: Args) -> Result<()> {
    Err(eyre!(
        "Raw input capture currently requires a Unix-like environment."
    ))
}

#[cfg(unix)]
fn process_event_bytes(bytes: Vec<u8>, events: &mut Vec<InputEventInfo>, count: &mut usize) {
    if bytes.is_empty() {
        return;
    }
    let info = InputEventInfo::from_bytes(bytes);
    events.push(info);
    *count += 1;
}

fn format_event_info(info: &InputEventInfo) -> Row<'static> {
    let description = if info.guess.description.is_empty() {
        String::new()
    } else {
        info.guess.description.clone()
    };

    Row::new(vec![
        Cell::from(info.hex_string.clone()).style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Cell::from(info.escaped_string.clone()).style(Style::default().fg(Color::Cyan)),
        Cell::from(info.guess.key.clone()).style(
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Cell::from(info.guess.code.clone()).style(Style::default().fg(Color::Blue)),
        Cell::from(info.guess.modifiers.clone()).style(Style::default().fg(Color::Magenta)),
        Cell::from(info.guess.kind.clone()).style(Style::default().fg(Color::Red)),
        Cell::from(description).style(Style::default().fg(Color::White)),
    ])
}

impl InputEventInfo {
    fn from_bytes(raw_bytes: Vec<u8>) -> Self {
        let guess = GuessInfo::from_bytes(&raw_bytes);
        let hex_string = raw_bytes
            .iter()
            .map(|b| format!("{:02X}", b))
            .collect::<Vec<_>>()
            .join(" ");
        let escaped_string = escape_bytes(&raw_bytes);
        Self {
            hex_string,
            escaped_string,
            guess,
        }
    }
}

impl GuessInfo {
    fn from_bytes(bytes: &[u8]) -> Self {
        match interpret_bytes(bytes) {
            Some(interp) => GuessInfo {
                key: interp.key_display,
                code: format!("{:?}", interp.code),
                modifiers: format_modifiers(interp.modifiers),
                kind: "Press".to_string(),
                description: interp.description,
            },
            None => GuessInfo {
                key: "Unknown".to_string(),
                code: "Unknown".to_string(),
                modifiers: "None".to_string(),
                kind: "Unknown".to_string(),
                description: String::new(),
            },
        }
    }
}

struct KeyInterpretation {
    key_display: String,
    code: KeyCode,
    modifiers: KeyModifiers,
    description: String,
}

fn interpret_bytes(bytes: &[u8]) -> Option<KeyInterpretation> {
    if bytes.is_empty() {
        return None;
    }

    interpret_csi_sequence(bytes)
        .or_else(|| interpret_ss3_sequence(bytes))
        .or_else(|| interpret_alt_sequence(bytes))
        .or_else(|| interpret_single_byte(bytes))
        .or_else(|| interpret_utf8_char(bytes))
}

fn interpret_single_byte(bytes: &[u8]) -> Option<KeyInterpretation> {
    if bytes.len() != 1 {
        return None;
    }
    let byte = bytes[0];
    let description;
    let (code, key_display, modifiers) = match byte {
        0x00 => (KeyCode::Null, "Null".to_string(), KeyModifiers::empty()),
        b'\r' | b'\n' => (KeyCode::Enter, "Enter".to_string(), KeyModifiers::empty()),
        b'\t' => (KeyCode::Tab, "Tab".to_string(), KeyModifiers::empty()),
        0x7F => (
            KeyCode::Backspace,
            "Backspace".to_string(),
            KeyModifiers::empty(),
        ),
        0x08 => (
            KeyCode::Backspace,
            "Backspace".to_string(),
            KeyModifiers::CONTROL,
        ),
        0x1B => (KeyCode::Esc, "Esc".to_string(), KeyModifiers::empty()),
        0x01..=0x1A => {
            let ch = (byte + 0x60) as char;
            (
                KeyCode::Char(ch),
                format!("'{}'", ch),
                KeyModifiers::CONTROL,
            )
        }
        0x1C..=0x1F => {
            let ch = (byte + 0x60) as char;
            (
                KeyCode::Char(ch),
                format!("'{}'", ch),
                KeyModifiers::CONTROL,
            )
        }
        0x20..=0x7E => {
            let ch = byte as char;
            (
                KeyCode::Char(ch),
                format!("'{}'", ch),
                KeyModifiers::empty(),
            )
        }
        _ => return None,
    };

    description = match code {
        KeyCode::Backspace if modifiers.contains(KeyModifiers::CONTROL) => {
            "Backspace (Ctrl+H)".to_string()
        }
        KeyCode::Char(_) if modifiers.contains(KeyModifiers::CONTROL) => {
            "Control-modified character".to_string()
        }
        KeyCode::Enter => "Carriage return".to_string(),
        KeyCode::Tab => "Horizontal tab".to_string(),
        KeyCode::Esc => "Escape".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Null => "NULL".to_string(),
        KeyCode::Char(_) => "Printable character".to_string(),
        _ => String::new(),
    };

    Some(KeyInterpretation {
        key_display,
        code,
        modifiers,
        description,
    })
}

fn interpret_utf8_char(bytes: &[u8]) -> Option<KeyInterpretation> {
    let width = utf8_char_width(*bytes.first()?);
    if width != bytes.len() {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let mut chars = text.chars();
    let ch = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some(KeyInterpretation {
        key_display: format!("'{}'", ch),
        code: KeyCode::Char(ch),
        modifiers: KeyModifiers::empty(),
        description: "UTF-8 character".to_string(),
    })
}

fn interpret_alt_sequence(bytes: &[u8]) -> Option<KeyInterpretation> {
    if bytes.len() < 2 || bytes[0] != 0x1B {
        return None;
    }
    let seq = &bytes[1..];
    let text = std::str::from_utf8(seq).ok()?;
    if text.is_empty() {
        return None;
    }
    let mut chars = text.chars();
    let ch = chars.next()?;
    if chars.next().is_some() {
        return None;
    }

    Some(KeyInterpretation {
        key_display: format!("'{}'", ch),
        code: KeyCode::Char(ch),
        modifiers: KeyModifiers::ALT,
        description: "Alt-modified character".to_string(),
    })
}

fn interpret_ss3_sequence(bytes: &[u8]) -> Option<KeyInterpretation> {
    if bytes.len() != 3 || bytes[0] != 0x1B || bytes[1] != b'O' {
        return None;
    }
    let final_byte = bytes[2] as char;
    let (code, key_display, description) = match final_byte {
        'P' => (
            KeyCode::F(1),
            "F1".to_string(),
            "SS3 function key".to_string(),
        ),
        'Q' => (
            KeyCode::F(2),
            "F2".to_string(),
            "SS3 function key".to_string(),
        ),
        'R' => (
            KeyCode::F(3),
            "F3".to_string(),
            "SS3 function key".to_string(),
        ),
        'S' => (
            KeyCode::F(4),
            "F4".to_string(),
            "SS3 function key".to_string(),
        ),
        'A' => (KeyCode::Up, "Up".to_string(), "SS3 arrow key".to_string()),
        'B' => (
            KeyCode::Down,
            "Down".to_string(),
            "SS3 arrow key".to_string(),
        ),
        'C' => (
            KeyCode::Right,
            "Right".to_string(),
            "SS3 arrow key".to_string(),
        ),
        'D' => (
            KeyCode::Left,
            "Left".to_string(),
            "SS3 arrow key".to_string(),
        ),
        'H' => (
            KeyCode::Home,
            "Home".to_string(),
            "SS3 home key".to_string(),
        ),
        'F' => (KeyCode::End, "End".to_string(), "SS3 end key".to_string()),
        _ => return None,
    };

    Some(KeyInterpretation {
        key_display,
        code,
        modifiers: KeyModifiers::empty(),
        description,
    })
}

fn interpret_csi_sequence(bytes: &[u8]) -> Option<KeyInterpretation> {
    let (final_byte, params) = parse_csi(bytes)?;
    match final_byte {
        'A' => Some(build_arrow_guess("Up", KeyCode::Up, &params)),
        'B' => Some(build_arrow_guess("Down", KeyCode::Down, &params)),
        'C' => Some(build_arrow_guess("Right", KeyCode::Right, &params)),
        'D' => Some(build_arrow_guess("Left", KeyCode::Left, &params)),
        'F' => Some(build_arrow_guess("End", KeyCode::End, &params)),
        'H' => Some(build_arrow_guess("Home", KeyCode::Home, &params)),
        'Z' => {
            let modifiers = KeyModifiers::SHIFT;
            Some(KeyInterpretation {
                key_display: "BackTab".to_string(),
                code: KeyCode::BackTab,
                modifiers,
                description: "CSI BackTab sequence".to_string(),
            })
        }
        '~' => interpret_csi_tilde(&params),
        _ => None,
    }
}

fn build_arrow_guess(name: &str, code: KeyCode, params: &[u16]) -> KeyInterpretation {
    let (_, modifiers) = split_params_and_modifiers(params);
    KeyInterpretation {
        key_display: name.to_string(),
        code,
        modifiers,
        description: "CSI arrow/navigation sequence".to_string(),
    }
}

fn interpret_csi_tilde(params: &[u16]) -> Option<KeyInterpretation> {
    let (base, modifiers) = split_params_and_modifiers(params);
    let key_id = base.first().copied()?;
    let (code, key_display, description) = match key_id {
        1 | 7 => (
            KeyCode::Home,
            "Home".to_string(),
            "CSI ~ (Home)".to_string(),
        ),
        2 => (
            KeyCode::Insert,
            "Insert".to_string(),
            "CSI ~ (Insert)".to_string(),
        ),
        3 => (
            KeyCode::Delete,
            "Delete".to_string(),
            "CSI ~ (Delete)".to_string(),
        ),
        4 | 8 => (KeyCode::End, "End".to_string(), "CSI ~ (End)".to_string()),
        5 => (
            KeyCode::PageUp,
            "PageUp".to_string(),
            "CSI ~ (PageUp)".to_string(),
        ),
        6 => (
            KeyCode::PageDown,
            "PageDown".to_string(),
            "CSI ~ (PageDown)".to_string(),
        ),
        11 => (
            KeyCode::F(1),
            "F1".to_string(),
            "CSI ~ function key".to_string(),
        ),
        12 => (
            KeyCode::F(2),
            "F2".to_string(),
            "CSI ~ function key".to_string(),
        ),
        13 => (
            KeyCode::F(3),
            "F3".to_string(),
            "CSI ~ function key".to_string(),
        ),
        14 => (
            KeyCode::F(4),
            "F4".to_string(),
            "CSI ~ function key".to_string(),
        ),
        15 => (
            KeyCode::F(5),
            "F5".to_string(),
            "CSI ~ function key".to_string(),
        ),
        17 => (
            KeyCode::F(6),
            "F6".to_string(),
            "CSI ~ function key".to_string(),
        ),
        18 => (
            KeyCode::F(7),
            "F7".to_string(),
            "CSI ~ function key".to_string(),
        ),
        19 => (
            KeyCode::F(8),
            "F8".to_string(),
            "CSI ~ function key".to_string(),
        ),
        20 => (
            KeyCode::F(9),
            "F9".to_string(),
            "CSI ~ function key".to_string(),
        ),
        21 => (
            KeyCode::F(10),
            "F10".to_string(),
            "CSI ~ function key".to_string(),
        ),
        23 => (
            KeyCode::F(11),
            "F11".to_string(),
            "CSI ~ function key".to_string(),
        ),
        24 => (
            KeyCode::F(12),
            "F12".to_string(),
            "CSI ~ function key".to_string(),
        ),
        _ => return None,
    };

    Some(KeyInterpretation {
        key_display,
        code,
        modifiers,
        description,
    })
}

fn parse_csi(bytes: &[u8]) -> Option<(char, Vec<u16>)> {
    if bytes.len() < 3 || bytes[0] != 0x1B || bytes[1] != b'[' {
        return None;
    }
    let final_byte = *bytes.last()?;
    if !(0x40..=0x7E).contains(&final_byte) {
        return None;
    }
    let mut params_bytes = &bytes[2..bytes.len() - 1];
    while let Some(b'?') = params_bytes.first() {
        params_bytes = &params_bytes[1..];
    }

    if params_bytes.is_empty() {
        return Some((final_byte as char, Vec::new()));
    }

    let params_str = std::str::from_utf8(params_bytes).ok()?;
    let mut params = Vec::new();
    for part in params_str.split(';') {
        if part.is_empty() {
            continue;
        }
        if let Ok(value) = part.parse::<u16>() {
            params.push(value);
        } else {
            return None;
        }
    }

    Some((final_byte as char, params))
}

fn split_params_and_modifiers(params: &[u16]) -> (Vec<u16>, KeyModifiers) {
    if params.len() <= 1 {
        return (params.to_vec(), KeyModifiers::empty());
    }
    let (base, modifier_part) = params.split_at(params.len() - 1);
    let modifiers = decode_modifier_code(modifier_part[0]);
    (base.to_vec(), modifiers)
}

fn decode_modifier_code(value: u16) -> KeyModifiers {
    match value {
        2 => KeyModifiers::SHIFT,
        3 => KeyModifiers::ALT,
        4 => KeyModifiers::SHIFT | KeyModifiers::ALT,
        5 => KeyModifiers::CONTROL,
        6 => KeyModifiers::SHIFT | KeyModifiers::CONTROL,
        7 => KeyModifiers::ALT | KeyModifiers::CONTROL,
        8 => KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL,
        _ => KeyModifiers::empty(),
    }
}

fn format_modifiers(modifiers: KeyModifiers) -> String {
    if modifiers.is_empty() {
        "None".to_string()
    } else {
        format!("{:?}", modifiers)
    }
}

fn escape_bytes(bytes: &[u8]) -> String {
    let mut output = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'\x1B' => {
                output.push_str("\\x1B");
                i += 1;
            }
            b'\n' => {
                output.push_str("\\n");
                i += 1;
            }
            b'\r' => {
                output.push_str("\\r");
                i += 1;
            }
            b'\t' => {
                output.push_str("\\t");
                i += 1;
            }
            0x20..=0x7E => {
                output.push(b as char);
                i += 1;
            }
            _ => {
                let width = utf8_char_width(b);
                if width > 1 && i + width <= bytes.len() {
                    if let Ok(slice) = std::str::from_utf8(&bytes[i..i + width]) {
                        output.push_str(slice);
                        i += width;
                        continue;
                    }
                }
                output.push_str(&format!("\\x{:02X}", b));
                i += 1;
            }
        }
    }
    output
}

fn utf8_char_width(first_byte: u8) -> usize {
    if first_byte < 0x80 {
        1
    } else if first_byte >> 5 == 0b110 {
        2
    } else if first_byte >> 4 == 0b1110 {
        3
    } else if first_byte >> 3 == 0b11110 {
        4
    } else {
        1
    }
}

fn try_extract_event(buffer: &[u8]) -> Option<usize> {
    if buffer.is_empty() {
        return None;
    }
    let first = buffer[0];

    if first == 0x1B {
        if buffer.len() >= 2 {
            match buffer[1] {
                b'[' => return csi_sequence_length(buffer),
                b'O' => {
                    if buffer.len() >= 3 {
                        return Some(3);
                    }
                }
                _ => {
                    let width = utf8_char_width(buffer[1]);
                    if buffer.len() >= 1 + width {
                        return Some(1 + width);
                    }
                }
            }
        }
        return None;
    }

    if first >= 0x80 {
        let width = utf8_char_width(first);
        if buffer.len() >= width {
            return Some(width);
        }
        return None;
    }

    Some(1)
}

fn csi_sequence_length(buffer: &[u8]) -> Option<usize> {
    if buffer.len() < 3 {
        return None;
    }
    for (idx, byte) in buffer[2..].iter().enumerate() {
        if (0x40..=0x7E).contains(byte) {
            return Some(idx + 3);
        }
    }
    None
}

#[cfg(unix)]
struct RawInputReader {
    stdin: io::Stdin,
    fd: libc::c_int,
    buffer: Vec<u8>,
    ready: VecDeque<Vec<u8>>,
    last_byte_at: Option<Instant>,
    flush_timeout: Duration,
}

#[cfg(unix)]
impl RawInputReader {
    fn new(flush_timeout: Duration) -> io::Result<Self> {
        let stdin = io::stdin();
        let fd = stdin.as_raw_fd();
        Ok(Self {
            stdin,
            fd,
            buffer: Vec::new(),
            ready: VecDeque::new(),
            last_byte_at: None,
            flush_timeout,
        })
    }

    fn poll_next(&mut self, timeout: Duration) -> io::Result<Option<Vec<u8>>> {
        if let Some(event) = self.ready.pop_front() {
            return Ok(Some(event));
        }

        if !self.buffer.is_empty() && self.should_flush_pending() {
            let event = self.buffer.drain(..).collect();
            return Ok(Some(event));
        }

        let effective_timeout = self.effective_timeout(timeout);
        let mut fds = [PollFd::new(self.stdin.as_fd(), PollFlags::POLLIN)];
        let poll_timeout_ms = duration_to_poll_timeout(effective_timeout);
        let poll_timeout = PollTimeout::try_from(poll_timeout_ms).unwrap_or(PollTimeout::MAX);
        let res = loop {
            match poll(&mut fds, poll_timeout) {
                Ok(res) => break res,
                Err(Errno::EINTR) => {
                    tracing::warn!(
                        target: "raw_input_reader",
                        fd = self.fd,
                        timeout_ms = poll_timeout_ms,
                        buffer_len = self.buffer.len(),
                        last_byte_age_ms = self
                            .last_byte_at
                            .map(|instant| instant.elapsed().as_millis() as i64)
                            .unwrap_or(-1),
                        "poll interrupted, retrying"
                    );
                    continue;
                }
                Err(errno) => {
                    let errno_value = errno as i32;
                    let io_err: io::Error = errno.into();
                    tracing::error!(
                        target: "raw_input_reader",
                        fd = self.fd,
                        timeout_ms = poll_timeout_ms,
                        buffer_len = self.buffer.len(),
                        last_byte_age_ms = self
                            .last_byte_at
                            .map(|instant| instant.elapsed().as_millis() as i64)
                            .unwrap_or(-1),
                        errno = errno_value,
                        kind = ?io_err.kind(),
                        "poll failed"
                    );
                    return Err(io_err);
                }
            }
        };

        if res == 0 {
            if !self.buffer.is_empty() && self.should_flush_pending() {
                let event = self.buffer.drain(..).collect();
                return Ok(Some(event));
            }
            return Ok(None);
        }

        if let Some(revents) = fds[0].revents() {
            if revents.contains(PollFlags::POLLIN) {
                let mut byte = [0u8; 1];
                let mut stdin_lock = self.stdin.lock();
                loop {
                    match stdin_lock.read(&mut byte) {
                        Ok(0) => break,
                        Ok(_) => {
                            self.push_byte(byte[0]);
                            if let Some(event) = self.ready.pop_front() {
                                drop(stdin_lock);
                                return Ok(Some(event));
                            }
                        }
                        Err(err) if err.kind() == ErrorKind::Interrupted => continue,
                        Err(err) if err.kind() == ErrorKind::WouldBlock => break,
                        Err(err) => return Err(err),
                    }
                }
            }
        }

        if let Some(event) = self.ready.pop_front() {
            return Ok(Some(event));
        }

        if !self.buffer.is_empty() && self.should_flush_pending() {
            let event = self.buffer.drain(..).collect();
            return Ok(Some(event));
        }

        Ok(None)
    }

    fn push_byte(&mut self, byte: u8) {
        self.buffer.push(byte);
        self.last_byte_at = Some(Instant::now());
        while let Some(len) = try_extract_event(&self.buffer) {
            let event = self.buffer.drain(..len).collect::<Vec<u8>>();
            self.ready.push_back(event);
        }
    }

    fn should_flush_pending(&self) -> bool {
        self.last_byte_at
            .map(|instant| instant.elapsed() >= self.flush_timeout)
            .unwrap_or(false)
    }

    fn effective_timeout(&self, requested: Duration) -> Duration {
        if self.buffer.is_empty() {
            return requested;
        }

        if let Some(last) = self.last_byte_at {
            let elapsed = last.elapsed();
            if elapsed >= self.flush_timeout {
                Duration::ZERO
            } else {
                requested.min(self.flush_timeout - elapsed)
            }
        } else {
            requested
        }
    }
}

#[cfg(unix)]
fn duration_to_poll_timeout(duration: Duration) -> libc::c_int {
    if duration == Duration::ZERO {
        return 0;
    }

    let millis = duration.as_millis().min(i32::MAX as u128);
    millis as libc::c_int
}

// Copied from terminal.rs
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen},
};
use ratatui::{TerminalOptions, Viewport};

fn init_terminal(inline: bool, height: u16) -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    tracing::info!("Initializing terminal");

    enable_raw_mode().wrap_err("Failed to enable raw mode")?;

    let mut stdout = io::stdout();
    execute!(stdout, EnableMouseCapture).wrap_err("Failed to enable mouse capture")?;

    if !inline {
        tracing::debug!("Entering alternate screen mode");
        execute!(stdout, EnterAlternateScreen).wrap_err("Failed to enter alternate screen")?;
    } else {
        tracing::debug!("Using inline mode with height: {}", height);
    }

    // Set up panic hook
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = restore_terminal(inline, height);
        hook(panic_info);
    }));

    let backend = CrosstermBackend::new(stdout);

    let viewport = if inline {
        Viewport::Inline(height)
    } else {
        Viewport::Fullscreen
    };

    let mut terminal = ratatui::Terminal::with_options(backend, TerminalOptions { viewport })
        .wrap_err("Failed to create terminal")?;

    terminal.clear().wrap_err("Failed to clear terminal")?;
    terminal.hide_cursor().wrap_err("Failed to hide cursor")?;

    tracing::info!("Terminal initialized successfully");
    Ok(terminal)
}

fn restore_terminal(inline: bool, height: u16) -> io::Result<()> {
    tracing::info!("Restoring terminal");

    if let Err(e) = disable_raw_mode() {
        tracing::error!("Failed to disable raw mode during restore: {}", e);
    }

    let mut stdout = io::stdout();

    if let Err(e) = execute!(stdout, DisableMouseCapture) {
        tracing::error!("Failed to disable mouse capture during restore: {}", e);
    }

    if !inline {
        execute!(stdout, crossterm::terminal::LeaveAlternateScreen)?;
    } else {
        if let Ok((_cols, rows)) = crossterm::terminal::size() {
            execute!(
                stdout,
                crossterm::cursor::MoveTo(0, rows.saturating_sub(height)),
                crossterm::terminal::Clear(crossterm::terminal::ClearType::FromCursorDown),
                crossterm::cursor::Show
            )?;
        }
    }

    stdout.flush()?;

    tracing::info!("Terminal restore completed");
    Ok(())
}

/// Logger guard
struct LoggerGuard {
    _guard: tracing_appender::non_blocking::WorkerGuard,
}

/// Initialize logger
fn init_logger() -> Result<LoggerGuard> {
    let log_dir = get_log_directory();

    std::fs::create_dir_all(&log_dir).wrap_err("Failed to create log directory")?;

    let log_file = rolling::daily(&log_dir, "debug-keys.log");
    let (non_blocking_log_file, guard) = tracing_appender::non_blocking(log_file);

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking_log_file)
        .with_ansi(false)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_file(true)
        .with_line_number(true)
        .with_target(true);

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_file(true)
        .with_line_number(true)
        .with_target(true);

    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer)
        .try_init()
        .wrap_err("Failed to initialize tracing subscriber")?;

    tracing::info!("Logger initialized to: {}", log_dir.display());
    Ok(LoggerGuard { _guard: guard })
}

fn get_log_directory() -> PathBuf {
    if let Ok(dir) = std::env::var("TERMEVENTS_LOG_DIR") {
        PathBuf::from(dir)
    } else if let Some(home) = dirs::home_dir() {
        home.join(".termevents").join("logs")
    } else {
        PathBuf::from("/tmp/termevents")
    }
}
