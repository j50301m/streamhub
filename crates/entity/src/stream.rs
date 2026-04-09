use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
pub enum StreamStatus {
    #[sea_orm(string_value = "Pending")]
    Pending,
    #[sea_orm(string_value = "Live")]
    Live,
    #[sea_orm(string_value = "Ended")]
    Ended,
    #[sea_orm(string_value = "Error")]
    Error,
}

// NOTE: #[sea_orm::model] dense format macro is the target for SeaORM 2.0,
// but may not be available in rc.37. If it fails to compile, we keep the
// explicit DeriveEntityModel + empty Relation enum as a compatible fallback.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "streams")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(unique)]
    pub stream_key: String,
    #[sea_orm(nullable)]
    pub title: Option<String>,
    pub status: StreamStatus,
    #[sea_orm(nullable)]
    pub started_at: Option<ChronoDateTimeUtc>,
    #[sea_orm(nullable)]
    pub ended_at: Option<ChronoDateTimeUtc>,
    pub created_at: ChronoDateTimeUtc,
}

impl ActiveModelBehavior for ActiveModel {}
