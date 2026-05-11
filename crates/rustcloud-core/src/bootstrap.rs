// Implemented in Task 13.

#[derive(Default)]
pub struct BootstrapRegistry;
pub type BootstrapHook = Box<dyn Send>;

#[allow(dead_code)]
pub fn boxed_hook() -> BootstrapHook {
    Box::new(())
}
