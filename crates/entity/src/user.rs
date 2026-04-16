use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    #[sea_orm(string_value = "Broadcaster")]
    Broadcaster,
    #[sea_orm(string_value = "Viewer")]
    Viewer,
    #[sea_orm(string_value = "Admin")]
    Admin,
}

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "users")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(unique)]
    pub email: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub role: UserRole,
    #[serde(default)]
    #[sea_orm(default_value = "false")]
    pub is_suspended: bool,
    pub suspended_until: Option<ChronoDateTimeUtc>,
    pub suspension_reason: Option<String>,
    pub created_at: ChronoDateTimeUtc,
}

impl ActiveModelBehavior for ActiveModel {}
