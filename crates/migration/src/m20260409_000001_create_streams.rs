use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Entity-first: generate DDL from entity definition instead of hand-writing columns.
        let backend = manager.get_database_backend();
        let schema = sea_orm::Schema::new(backend);
        let mut stmt = schema.create_table_from_entity(streamhub_entity::stream::Entity);
        manager.create_table(stmt.if_not_exists().to_owned()).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(streamhub_entity::stream::Entity)
                    .to_owned(),
            )
            .await
    }
}
