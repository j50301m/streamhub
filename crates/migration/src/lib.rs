//! SeaORM migrator. Schema is synced entity-first at startup; this migrator
//! is reserved for seed data and future schema-only changes not expressible
//! via entity reflection.
#![warn(missing_docs)]

pub use sea_orm_migration::prelude::*;

/// Migrator entry point consumed by the `migration` binary.
pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![]
    }
}
