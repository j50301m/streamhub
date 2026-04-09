pub use sea_orm_migration::prelude::*;

mod m20260409_000001_create_streams;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(m20260409_000001_create_streams::Migration)]
    }
}
