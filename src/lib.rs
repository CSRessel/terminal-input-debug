#![allow(dead_code)]

use color_eyre::Result;
use crossterm::{
    cursor,
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, size, Clear, ClearType, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use eyre::WrapErr;
use ratatui::{backend::CrosstermBackend, Terminal, TerminalOptions, Viewport};
use std::io::{self, Write};
use std::path::PathBuf;
use tracing_appender::rolling;
use tracing_subscriber::{self, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Selectable writer that can target stdout or stderr interchangeably.
pub enum TerminalWriter {
    Stdout(io::Stdout),
    Stderr(io::Stderr),
}

impl TerminalWriter {
    fn stdout() -> Self {
        Self::Stdout(io::stdout())
    }

    fn stderr() -> Self {
        Self::Stderr(io::stderr())
    }
}

impl Write for TerminalWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Stdout(writer) => writer.write(buf),
            Self::Stderr(writer) => writer.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Stdout(writer) => writer.flush(),
            Self::Stderr(writer) => writer.flush(),
        }
    }
}

/// Selects which stream the alternate screen backend should target.
#[derive(Debug, Clone, Copy)]
pub enum AlternateScreenBackend {
    Stdout,
    Stderr,
}

impl AlternateScreenBackend {
    fn into_writer(self) -> TerminalWriter {
        match self {
            Self::Stdout => TerminalWriter::stdout(),
            Self::Stderr => TerminalWriter::stderr(),
        }
    }
}

/// Describes how the TUI consumes terminal real estate.
#[derive(Debug, Clone, Copy)]
pub enum ViewportMode {
    Inline { height: u16 },
    AlternateScreen { backend: AlternateScreenBackend },
}

impl Default for ViewportMode {
    fn default() -> Self {
        Self::AlternateScreen {
            backend: AlternateScreenBackend::Stdout,
        }
    }
}

impl ViewportMode {
    fn is_inline(self) -> bool {
        matches!(self, Self::Inline { .. })
    }

    fn inline_height(self) -> Option<u16> {
        match self {
            Self::Inline { height } => Some(height),
            Self::AlternateScreen { .. } => None,
        }
    }

    fn writer(self) -> TerminalWriter {
        match self {
            Self::Inline { .. } => TerminalWriter::stdout(),
            Self::AlternateScreen { backend } => backend.into_writer(),
        }
    }
}

/// Logger guard
struct LoggerGuard {
    _guard: tracing_appender::non_blocking::WorkerGuard,
}

/// Initialize logger
fn init_file_logger(app_name: &str) -> Result<LoggerGuard> {
    let log_dir = get_log_directory(app_name);

    std::fs::create_dir_all(&log_dir).wrap_err("Failed to create log directory")?;

    let log_file = rolling::daily(&log_dir, "logs");
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

    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .try_init()
        .wrap_err("Failed to initialize tracing subscriber")?;

    tracing::debug!("Logger initialized to: {}", log_dir.display());
    Ok(LoggerGuard { _guard: guard })
}

fn get_log_directory(app_name: &str) -> PathBuf {
    let env_var = format!("{}_LOG_DIR", app_name.to_ascii_uppercase());

    if let Ok(dir) = std::env::var(&env_var) {
        PathBuf::from(dir)
    } else if let Some(home) = dirs::home_dir() {
        home.join(format!(".{}", app_name)).join("logs")
    } else {
        PathBuf::from("/tmp").join(app_name)
    }
}

fn init_terminal(
    viewport_mode: ViewportMode,
    use_panic_terminal_restore: bool,
    capture_mouse: bool,
    hide_cursor: bool,
) -> Result<Terminal<CrosstermBackend<TerminalWriter>>> {
    tracing::debug!("Initializing terminal");

    enable_raw_mode().wrap_err("Failed to enable raw mode")?;

    let mut terminal_output = viewport_mode.writer();
    if capture_mouse {
        execute!(terminal_output, EnableMouseCapture).wrap_err("Failed to enable mouse capture")?;
    }

    if !viewport_mode.is_inline() {
        tracing::debug!("Entering alternate screen mode");
        execute!(terminal_output, EnterAlternateScreen)
            .wrap_err("Failed to enter alternate screen")?;
    } else {
        if let Some(height) = viewport_mode.inline_height() {
            tracing::debug!("Using inline mode with height: {}", height);
        }
    }

    // Set up panic hook
    if use_panic_terminal_restore {
        let panic_viewport = viewport_mode;
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            // We've already panicked so ignore any err
            let _ = restore_terminal(capture_mouse, hide_cursor, panic_viewport);
            hook(panic_info);
        }));
    }

    let backend = CrosstermBackend::new(terminal_output);

    let viewport = match viewport_mode {
        ViewportMode::Inline { height } => Viewport::Inline(height),
        ViewportMode::AlternateScreen { .. } => Viewport::Fullscreen,
    };

    let mut terminal = ratatui::Terminal::with_options(backend, TerminalOptions { viewport })
        .wrap_err("Failed to create terminal")?;

    terminal.clear().wrap_err("Failed to clear terminal")?;
    if hide_cursor {
        terminal.hide_cursor().wrap_err("Failed to hide cursor")?;
    }

    tracing::debug!("Terminal initialized successfully");
    Ok(terminal)
}

fn restore_terminal(
    capture_mouse: bool,
    hide_cursor: bool,
    viewport_mode: ViewportMode,
) -> io::Result<()> {
    tracing::debug!("Restoring terminal");

    if let Err(e) = disable_raw_mode() {
        tracing::error!("Failed to disable raw mode during restore: {}", e);
    }

    let mut stdout = io::stdout();

    if capture_mouse {
        if let Err(e) = execute!(stdout, DisableMouseCapture) {
            tracing::error!("Failed to disable mouse capture during restore: {}", e);
        }
    }

    if !viewport_mode.is_inline() {
        execute!(stdout, LeaveAlternateScreen)?;
    } else {
        if let Some(height) = viewport_mode.inline_height() {
            if let Ok((_cols, rows)) = size() {
                execute!(
                    stdout,
                    cursor::MoveTo(0, rows.saturating_sub(height)),
                    Clear(ClearType::FromCursorDown),
                )?;
            }
        }
    }

    if hide_cursor {
        execute!(stdout, cursor::Show)?;
    }
    stdout.flush()?;

    tracing::debug!("Terminal restore completed");
    Ok(())
}

/// Coordinates color-eyre, logging, and terminal lifecycle for the TUI.
#[derive(Debug, Clone)]
pub struct TuiAppBuilder {
    app_name: String,
    use_panic_terminal_restore: bool,
    use_color_eyre: bool,
    use_disk_logs: bool,
    capture_mouse: bool,
    hide_cursor: bool,
    viewport: ViewportMode,
}

impl Default for TuiAppBuilder {
    fn default() -> Self {
        Self {
            app_name: String::new(),
            use_panic_terminal_restore: true,
            use_color_eyre: true,
            use_disk_logs: true,
            capture_mouse: true,
            hide_cursor: true,
            viewport: ViewportMode::default(),
        }
    }
}

impl TuiAppBuilder {
    pub fn new(app_name: impl Into<String>) -> Self {
        Self {
            app_name: app_name.into(),
            ..Self::default()
        }
    }

    pub fn app_name(mut self, app_name: impl Into<String>) -> Self {
        self.app_name = app_name.into();
        self
    }

    pub fn capture_mouse(mut self, capture_mouse: bool) -> Self {
        self.capture_mouse = capture_mouse;
        self
    }

    pub fn hide_cursor(mut self, hide_cursor: bool) -> Self {
        self.hide_cursor = hide_cursor;
        self
    }

    pub fn inline(mut self, height: u16) -> Self {
        self.viewport = ViewportMode::Inline { height };
        self
    }

    pub fn alternate_screen(mut self) -> Self {
        self.viewport = ViewportMode::AlternateScreen {
            backend: AlternateScreenBackend::Stdout,
        };
        self
    }

    pub fn alternate_screen_backend(mut self, backend: AlternateScreenBackend) -> Self {
        self.viewport = ViewportMode::AlternateScreen { backend };
        self
    }

    pub fn use_panic_terminal_restore(mut self, use_panic_terminal_restore: bool) -> Self {
        self.use_panic_terminal_restore = use_panic_terminal_restore;
        self
    }

    pub fn use_color_eyre(mut self, use_color_eyre: bool) -> Self {
        self.use_color_eyre = use_color_eyre;
        self
    }

    pub fn use_disk_logs(mut self, use_disk_logs: bool) -> Self {
        self.use_disk_logs = use_disk_logs;
        self
    }

    pub fn build(self) -> TuiApp {
        let app_name = if self.app_name.is_empty() {
            env!("CARGO_PKG_NAME").to_string()
        } else {
            self.app_name
        };

        TuiApp {
            logger_guard: None,
            app_name,
            use_panic_terminal_restore: self.use_panic_terminal_restore,
            use_color_eyre: self.use_color_eyre,
            use_disk_logs: self.use_disk_logs,
            capture_mouse: self.capture_mouse,
            hide_cursor: self.hide_cursor,
            viewport: self.viewport,
        }
    }
}

pub struct TuiApp {
    logger_guard: Option<LoggerGuard>,
    app_name: String,
    use_panic_terminal_restore: bool,
    use_color_eyre: bool,
    use_disk_logs: bool,
    capture_mouse: bool,
    hide_cursor: bool,
    viewport: ViewportMode,
}

impl TuiApp {
    // TODO customization points:
    //
    // Terminal Lifecycle
    //
    // - Inline mode currently forces stdout. Re-evaluate whether to support directing inline output
    //   elsewhere without breaking existing guarantees.
    // - tui_core.rs:128-137 hard-codes clearing the inline viewport on restore; provide options
    //   for inline mode restore policies such as “leave inline buffer untouched”, “clear bottom N
    //   lines”, or “always clear everything”
    // - The user should be able to specify welcome, goodbye, and error banners that are printed
    //   in those respective situations, either entirely before or entirely after all the remaining
    //   terminal lifecycle management.
    pub fn builder(app_name: impl Into<String>) -> TuiAppBuilder {
        TuiAppBuilder::new(app_name)
    }

    /// Construct a new application harness with default settings.
    pub fn new(app_name: impl Into<String>) -> Self {
        Self::builder(app_name).build()
    }

    /// Access the application name currently configured for the TUI.
    pub fn app_name(&self) -> &str {
        &self.app_name
    }

    /// Install diagnostics, start logging, and return a ready-to-draw terminal.
    pub fn init(&mut self) -> Result<Terminal<CrosstermBackend<TerminalWriter>>> {
        if self.use_color_eyre {
            color_eyre::install().expect("Failed to install color-eyre");
        }

        if self.use_disk_logs {
            self.logger_guard =
                Some(init_file_logger(self.app_name()).expect("Failed to initialize file logger"));
        } else {
            self.logger_guard = None;
        }

        init_terminal(
            self.viewport,
            self.use_panic_terminal_restore,
            self.capture_mouse,
            self.hide_cursor,
        )
    }

    /// Restore the terminal to its pre-initialization state.
    pub fn restore(&self) -> io::Result<()> {
        restore_terminal(self.capture_mouse, self.hide_cursor, self.viewport)
    }
}
