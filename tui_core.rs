// #![allow(dead_code)]

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
    capture_mouse: bool,
    hide_cursor: bool,
    inline: bool,
    inline_height: u16,
) -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    tracing::debug!("Initializing terminal");

    enable_raw_mode().wrap_err("Failed to enable raw mode")?;

    let mut stdout = io::stdout();
    if capture_mouse {
        execute!(stdout, EnableMouseCapture).wrap_err("Failed to enable mouse capture")?;
    }

    if !inline {
        tracing::debug!("Entering alternate screen mode");
        execute!(stdout, EnterAlternateScreen).wrap_err("Failed to enter alternate screen")?;
    } else {
        tracing::debug!("Using inline mode with height: {}", inline_height);
    }

    // Set up panic hook
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        // We've already panicked so ignore any err
        let _ = restore_terminal(capture_mouse, hide_cursor, inline, inline_height);
        hook(panic_info);
    }));

    let backend = CrosstermBackend::new(stdout);

    let viewport = if inline {
        Viewport::Inline(inline_height)
    } else {
        Viewport::Fullscreen
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
    inline: bool,
    inline_height: u16,
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

    if !inline {
        execute!(stdout, LeaveAlternateScreen)?;
    } else {
        if let Ok((_cols, rows)) = size() {
            execute!(
                stdout,
                cursor::MoveTo(0, rows.saturating_sub(inline_height)),
                Clear(ClearType::FromCursorDown),
            )?;
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
    use_backend_stdout: bool,
    use_color_eyre: bool,
    use_tracing: bool,
    use_disk_logs: bool,
    capture_mouse: bool,
    hide_cursor: bool,
    inline: bool,
    inline_height: u16,
}

impl Default for TuiAppBuilder {
    fn default() -> Self {
        Self {
            app_name: String::new(),
            use_backend_stdout: true,
            use_color_eyre: true,
            use_tracing: true,
            use_disk_logs: true,
            capture_mouse: true,
            hide_cursor: true,
            inline: false,
            inline_height: 0,
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

    pub fn inline(mut self, inline: bool) -> Self {
        self.inline = inline;
        self
    }

    pub fn inline_height(mut self, inline_height: u16) -> Self {
        self.inline_height = inline_height;
        self
    }

    pub fn use_backend_stdout(mut self, use_backend_stdout: bool) -> Self {
        self.use_backend_stdout = use_backend_stdout;
        self
    }

    pub fn use_color_eyre(mut self, use_color_eyre: bool) -> Self {
        self.use_color_eyre = use_color_eyre;
        self
    }

    pub fn use_tracing(mut self, use_tracing: bool) -> Self {
        self.use_tracing = use_tracing;
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
            use_backend_stdout: self.use_backend_stdout,
            use_color_eyre: self.use_color_eyre,
            use_tracing: self.use_tracing,
            use_disk_logs: self.use_disk_logs,
            capture_mouse: self.capture_mouse,
            hide_cursor: self.hide_cursor,
            inline: self.inline,
            inline_height: self.inline_height,
        }
    }
}

pub struct TuiApp {
    logger_guard: Option<LoggerGuard>,
    app_name: String,
    use_backend_stdout: bool, // TODO
    use_color_eyre: bool,
    use_tracing: bool, // TODO
    use_disk_logs: bool,
    capture_mouse: bool,
    hide_cursor: bool,
    inline: bool,
    inline_height: u16,
}

impl TuiApp {
    // TODO customization points:
    //
    // Terminal Lifecycle
    //
    // - tui_core.rs:81 always enables mouse capture; expose a toggle so library users can opt out
    //   entirely.
    // - tui_core.rs:80 and tui_core:98 always uses stdout; allow the user to choose stderr instead
    //   if they want to preserve stdout for command output, e.g. for piping between command line
    //   tools (like fzf does, for example).
    // - tui_core.rs:109 hides the cursor until shutdown; add a hide_cursor flag for workflows that
    //   want to change cursor visibility.
    // - tui_core.rs:128-137 hard-codes clearing the inline viewport on restore; provide options
    //   for inline mode restore policies such as “leave inline buffer untouched”, “clear bottom N
    //   lines”, or “always clear everything”
    // - tui_core.rs:140-150 sets a panic hook that restores the terminal; allow callers to opt out
    //   of this behavior if they want to manage panics themselves.
    // - The user should be able to specify welcome, goodbye, and error banners that are printed
    //   in those respective situations, either entirely before or entirely after all the remaining
    //   terminal lifecycle management.
    //
    // Diagnostics & Logging
    //
    // - tui_core.rs:175-179 installs color_eyre and the tracer on every init; provide knobs to
    //   disable color_eyre, defer tracing setup to the host app, or accept custom subscriber
    //   builders.
    // - tui_core.rs:32-57 always builds both file and stderr layers with a fixed EnvFilter; expose
    //   configuration for log targets (file/stderr/none), ANSI usage, level filters, or to inject
    //   a prebuilt registry. E.g. user should be able to:
    //   ```rust
    //       let stderr_layer = tracing_subscriber::fmt::layer()
    //           .with_writer(std::io::stderr)
    //           .with_ansi(true)
    //           .with_thread_ids(true)
    //           .with_thread_names(true)
    //           .with_file(true)
    //           .with_line_number(true)
    //           .with_target(true);
    //   ```
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
    pub fn init(&mut self) -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
        if self.use_color_eyre {
            color_eyre::install().expect("Failed to install color-eyre");
        }

        if self.use_tracing && self.use_disk_logs {
            self.logger_guard =
                Some(init_file_logger(self.app_name()).expect("Failed to initialize file logger"));
        } else {
            self.logger_guard = None;
        }

        init_terminal(
            self.capture_mouse,
            self.hide_cursor,
            self.inline,
            self.inline_height,
        )
    }

    /// Restore the terminal to its pre-initialization state.
    pub fn restore(&self) -> io::Result<()> {
        restore_terminal(
            self.capture_mouse,
            self.hide_cursor,
            self.inline,
            self.inline_height,
        )
    }
}
