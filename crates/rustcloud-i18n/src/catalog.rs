// Implemented in Task 5.

#[derive(Debug)]
pub struct Catalog;

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("placeholder")]
    Placeholder,
}
