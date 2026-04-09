use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "recordings")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(
        belongs_to = "super::stream::Entity",
        from = "Column::StreamId",
        to = "super::stream::Column::Id"
    )]
    pub stream_id: Uuid,
    pub file_path: String,
    #[sea_orm(nullable)]
    pub duration_secs: Option<i64>,
    #[sea_orm(nullable)]
    pub file_size_bytes: Option<i64>,
    pub created_at: ChronoDateTimeUtc,
}

impl ActiveModelBehavior for ActiveModel {}
