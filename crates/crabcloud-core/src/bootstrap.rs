//! Bootstrap-hook registry. Apps register a future-producing closure here;
//! `AppStateBuilder::build()` drains the registry and runs each hook in order.

use crate::error::CoreResult;
use crate::state::AppState;
use std::future::Future;
use std::pin::Pin;

/// A registered bootstrap action. Receives an owned `AppState` clone (cheap —
/// `AppState` is `Arc`-backed) for setup (registering capability providers,
/// running migrations, seeding config). Returns a future that resolves when
/// setup is complete.
///
/// Use the `boxed_hook` helper below to wrap an ergonomic async closure
/// instead of authoring the `Pin<Box<...>>` coercion by hand.
pub type BootstrapHook =
    Box<dyn FnOnce(AppState) -> Pin<Box<dyn Future<Output = CoreResult<()>> + Send>> + Send>;

/// Wrap an `async` closure into a `BootstrapHook`. The closure takes the
/// `AppState` by value (clone-cheap) and returns any future of `CoreResult<()>`.
///
/// ```ignore
/// let hook = boxed_hook(|state| async move {
///     state.appconfig.set("core", "ready", "1").await?;
///     Ok(())
/// });
/// ```
pub fn boxed_hook<F, Fut>(f: F) -> BootstrapHook
where
    F: FnOnce(AppState) -> Fut + Send + 'static,
    Fut: Future<Output = CoreResult<()>> + Send + 'static,
{
    Box::new(move |state| Box::pin(f(state)))
}

/// Holds pending hooks. Cleared as hooks run during `AppStateBuilder::build`.
#[derive(Default)]
pub struct BootstrapRegistry {
    hooks: Vec<BootstrapHook>,
}

impl BootstrapRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    /// Append `hook` to the registry. Hooks fire in registration order.
    pub fn register(&mut self, hook: BootstrapHook) {
        self.hooks.push(hook);
    }

    /// Number of pending hooks.
    pub fn len(&self) -> usize {
        self.hooks.len()
    }

    /// Whether the registry has no pending hooks.
    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    /// Drains and runs all registered hooks in registration order.
    /// Each hook receives a fresh `AppState` clone.
    pub async fn run(&mut self, state: &AppState) -> CoreResult<()> {
        let hooks = std::mem::take(&mut self.hooks);
        for hook in hooks {
            hook(state.clone()).await?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for BootstrapRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BootstrapRegistry")
            .field("hooks", &self.hooks.len())
            .finish()
    }
}
