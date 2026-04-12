use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "lowercase")]
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

#[derive(Debug, Clone, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "lowercase")]
pub enum VodStatus {
    #[sea_orm(string_value = "None")]
    None,
    #[sea_orm(string_value = "Processing")]
    Processing,
    #[sea_orm(string_value = "Ready")]
    Ready,
    #[sea_orm(string_value = "Failed")]
    Failed,
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
    /// Owner user. Nullable for streams created before auth was added.
    #[sea_orm(nullable)]
    pub user_id: Option<Uuid>,
    #[sea_orm(unique)]
    pub stream_key: String,
    #[sea_orm(nullable)]
    pub title: Option<String>,
    pub status: StreamStatus,
    #[sea_orm(default_value = "None")]
    pub vod_status: VodStatus,
    #[sea_orm(nullable)]
    pub started_at: Option<ChronoDateTimeUtc>,
    #[sea_orm(nullable)]
    pub ended_at: Option<ChronoDateTimeUtc>,
    pub created_at: ChronoDateTimeUtc,
    /// HLS playlist URL for VOD playback (set after transcoding completes).
    #[sea_orm(nullable)]
    pub hls_url: Option<String>,
    /// Thumbnail image URL (set after transcoding extracts first frame).
    #[sea_orm(nullable)]
    pub thumbnail_url: Option<String>,
}

impl ActiveModelBehavior for ActiveModel {}
