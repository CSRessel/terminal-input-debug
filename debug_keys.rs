use clap::Parser;
use color_eyre::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent};
use eyre::WrapErr;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Terminal,
};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;
use tracing_appender::rolling;
use tracing_subscriber::{self, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

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
struct KeyEventInfo {
    key: String,
    code: String,
    modifiers: String,
    kind: String,
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

fn run(args: Args) -> Result<()> {
    // Simple ModelInit for inline mode
    let init = ModelInit::new(true);
    let height = 20;

    let mut terminal = init_terminal(&init, height)?;

    let mut events = Vec::new();
    let mut input_count = 0;

    let timeout_duration = Duration::from_secs(args.timeout);
    let start_time = Instant::now();

    loop {
        // Check timeout
        if start_time.elapsed() >= timeout_duration {
            break;
        }

        // Check max inputs
        if input_count >= args.max_inputs {
            break;
        }

        // Poll for event with short timeout
        if event::poll(Duration::from_millis(100))? {
            let event = event::read()?;
            if let Event::Key(key_event) = event {
                let info = format_key_event(key_event);
                events.push(info);
                input_count += 1;
            }
        }

        // Render
        terminal.draw(|f| {
            let size = f.area();
            let block = Block::default()
                .title("Debug Keys")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::Cyan));

            let inner_area = block.inner(size);
            f.render_widget(block, size);

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(2), Constraint::Min(1)])
                .split(inner_area);

            let status = format!(
                "Inputs: {}/{} | Time: {:.1}s / {}s",
                input_count,
                args.max_inputs,
                start_time.elapsed().as_secs_f32(),
                args.timeout
            );
            let status_para = Paragraph::new(status)
                .style(Style::default().fg(Color::Yellow))
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true });
            f.render_widget(status_para, chunks[0]);

            let events_text: Vec<Line> = events
                .iter()
                .rev()
                .take(50)
                .map(|info| {
                    Line::from(vec![
                        Span::styled("Key: ", Style::default().fg(Color::Green)),
                        Span::styled(
                            &info.key,
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(" | Code: ", Style::default().fg(Color::Blue)),
                        Span::styled(&info.code, Style::default().fg(Color::White)),
                        Span::styled(" | Mods: ", Style::default().fg(Color::Magenta)),
                        Span::styled(&info.modifiers, Style::default().fg(Color::White)),
                        Span::styled(" | Kind: ", Style::default().fg(Color::Red)),
                        Span::styled(&info.kind, Style::default().fg(Color::White)),
                    ])
                })
                .collect();

            let events_para = Paragraph::new(events_text).wrap(Wrap { trim: true });
            f.render_widget(events_para, chunks[1]);
        })?;
    }

    // Restore terminal
    restore_terminal(&init, height)?;

    Ok(())
}

fn format_key_event(event: KeyEvent) -> KeyEventInfo {
    let key = match event.code {
        KeyCode::Char(c) => format!("'{}'", c),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PageUp".to_string(),
        KeyCode::PageDown => "PageDown".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::BackTab => "BackTab".to_string(),
        KeyCode::Delete => "Delete".to_string(),
        KeyCode::Insert => "Insert".to_string(),
        KeyCode::F(n) => format!("F{}", n),
        KeyCode::Null => "Null".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::CapsLock => "CapsLock".to_string(),
        KeyCode::ScrollLock => "ScrollLock".to_string(),
        KeyCode::NumLock => "NumLock".to_string(),
        KeyCode::PrintScreen => "PrintScreen".to_string(),
        KeyCode::Pause => "Pause".to_string(),
        KeyCode::Menu => "Menu".to_string(),
        KeyCode::KeypadBegin => "KeypadBegin".to_string(),
        KeyCode::Media(_) => "Media".to_string(),
        KeyCode::Modifier(_) => "Modifier".to_string(),
    };

    let code = format!("{:?}", event.code);

    let modifiers = if event.modifiers.is_empty() {
        "None".to_string()
    } else {
        format!("{:?}", event.modifiers)
    };

    let kind = format!("{:?}", event.kind);

    KeyEventInfo {
        key,
        code,
        modifiers,
        kind,
    }
}

// Simple ModelInit
#[derive(Debug, Clone)]
struct ModelInit {
    init_inline_mode: bool,
}

impl ModelInit {
    fn new(inline_mode: bool) -> Self {
        Self {
            init_inline_mode: inline_mode,
        }
    }

    fn inline_mode(&self) -> bool {
        self.init_inline_mode
    }
}

// Copied from terminal.rs
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen},
};
use ratatui::{TerminalOptions, Viewport};

fn init_terminal(init: &ModelInit, height: u16) -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    tracing::info!(
        "Initializing terminal - inline_mode: {}",
        init.inline_mode()
    );

    enable_raw_mode().wrap_err("Failed to enable raw mode")?;

    let mut stdout = io::stdout();
    execute!(stdout, EnableMouseCapture).wrap_err("Failed to enable mouse capture")?;

    if !init.inline_mode() {
        tracing::debug!("Entering alternate screen mode");
        execute!(stdout, EnterAlternateScreen).wrap_err("Failed to enter alternate screen")?;
    } else {
        tracing::debug!("Using inline mode with height: {}", height);
    }

    // Set up panic hook
    let init_clone = init.clone();
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = restore_terminal(&init_clone, height);
        hook(panic_info);
    }));

    let backend = CrosstermBackend::new(stdout);

    let viewport = if init.inline_mode() {
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

fn restore_terminal(init: &ModelInit, height: u16) -> io::Result<()> {
    tracing::info!("Restoring terminal - inline_mode: {}", init.inline_mode());

    if let Err(e) = disable_raw_mode() {
        tracing::error!("Failed to disable raw mode during restore: {}", e);
    }

    let mut stdout = io::stdout();

    if let Err(e) = execute!(stdout, DisableMouseCapture) {
        tracing::error!("Failed to disable mouse capture during restore: {}", e);
    }

    if !init.inline_mode() {
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

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking_log_file)
        .with_ansi(false)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_file(true)
        .with_line_number(true)
        .with_target(true)
        .with_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("debug_keys=debug")),
        );

    tracing_subscriber::registry()
        .with(file_layer)
        .try_init()
        .wrap_err("Failed to initialize tracing subscriber")?;

    tracing::info!("Logger initialized to: {}", log_dir.display());
    Ok(LoggerGuard { _guard: guard })
}

fn get_log_directory() -> PathBuf {
    if let Ok(dir) = std::env::var("OPENCODE_LOG_DIR") {
        PathBuf::from(dir)
    } else if let Some(home) = dirs::home_dir() {
        home.join(".opencode").join("logs")
    } else {
        PathBuf::from("/tmp/opencode")
    }
}
