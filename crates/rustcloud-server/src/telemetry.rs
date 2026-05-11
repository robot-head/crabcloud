use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Initialize the global tracing subscriber.
///
/// - Filter defaults to `info`; `RUST_LOG` overrides if set.
/// - Output is JSON when stdout is not a TTY, plain otherwise.
pub fn init() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let is_tty = std::io::IsTerminal::is_terminal(&std::io::stdout());

    let registry = tracing_subscriber::registry().with(filter);

    if is_tty {
        registry.with(fmt::layer().with_target(false)).init();
    } else {
        registry.with(fmt::layer().json().with_target(true)).init();
    }
}
