//! User access-state check with Redis cache + DB fallback.
//!
//! Shared by `CurrentUser` (api), `AdminUser` (bo-api), and WS upgrade.

use cache::CacheStore;
use chrono::Utc;
use entity::user;
use sea_orm::{ActiveModelTrait, ConnectionTrait, EntityTrait, Set};
use uuid::Uuid;

/// Outcome of an access-state check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessState {
    /// User is active — request may proceed.
    Active,
    /// User is suspended — request must be rejected.
    Suspended,
}

/// Check whether a user is active or suspended, using Redis as a cache and
/// falling back to DB on cache miss.
///
/// Flow:
/// 1. `GET user:state:{user_id}` from Redis
/// 2. `"active"` → allow
/// 3. `"suspended"` → deny
/// 4. miss or unknown value → query DB `is_suspended`
///    - active → `SET user:state:{user_id} "active" EX 300` → allow
///    - suspended permanent → `SET user:state:{user_id} "suspended"` (no-expire) → deny
///    - suspended temporary → `SET user:state:{user_id} "suspended" EX remaining_secs` → deny
pub async fn load_user_access_state(
    cache: &dyn CacheStore,
    conn: &impl ConnectionTrait,
    user_id: Uuid,
) -> Result<AccessState, anyhow::Error> {
    let key = mediamtx::keys::user_state(&user_id);

    // 1. Check Redis cache
    if let Some(val) = cache.get(&key).await? {
        match val.as_str() {
            "active" => return Ok(AccessState::Active),
            "suspended" => return Ok(AccessState::Suspended),
            unknown => {
                // Unknown value — fall through to DB check (don't fail-open)
                tracing::warn!(
                    user_id = %user_id,
                    value = %unknown,
                    "unexpected user:state value, falling back to DB"
                );
            }
        }
    }

    // 2. Cache miss (or unknown value) — fallback to DB
    db_fallback(cache, conn, user_id, &key).await
}

/// Query DB for user suspension state and rehydrate the Redis cache.
async fn db_fallback(
    cache: &dyn CacheStore,
    conn: &impl ConnectionTrait,
    user_id: Uuid,
    key: &str,
) -> Result<AccessState, anyhow::Error> {
    let model = user::Entity::find_by_id(user_id)
        .one(conn)
        .await
        .map_err(anyhow::Error::from)?;

    let Some(model) = model else {
        // User not found — treat as suspended to block the request
        return Ok(AccessState::Suspended);
    };

    if !model.is_suspended {
        // Active user → cache with 300s TTL
        let _ = cache.set(key, "active", Some(300)).await;
        return Ok(AccessState::Active);
    }

    // User is suspended — check if temporary suspension has expired
    if let Some(until) = model.suspended_until {
        if Utc::now() >= until {
            // Suspension expired → lazy clear in DB
            let mut active: user::ActiveModel = model.into();
            active.is_suspended = Set(false);
            active.suspended_until = Set(None);
            active.suspension_reason = Set(None);
            let _ = active.update(conn).await;
            let _ = cache.set(key, "active", Some(300)).await;
            return Ok(AccessState::Active);
        }
        // Still suspended — cache with remaining TTL
        let remaining = (until - Utc::now()).num_seconds().max(1) as u64;
        let _ = cache.set(key, "suspended", Some(remaining)).await;
    } else {
        // Permanent suspension — no expiry
        let _ = cache.set(key, "suspended", None).await;
    }

    Ok(AccessState::Suspended)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cache::InMemoryCache;
    use chrono::{Duration, Utc};
    use entity::user;
    use sea_orm::{DbBackend, MockDatabase};

    fn active_user(id: Uuid) -> user::Model {
        user::Model {
            id,
            email: "test@example.com".to_string(),
            password_hash: String::new(),
            role: user::UserRole::Viewer,
            is_suspended: false,
            suspended_until: None,
            suspension_reason: None,
            created_at: Utc::now(),
        }
    }

    fn permanently_suspended_user(id: Uuid) -> user::Model {
        user::Model {
            id,
            email: "banned@example.com".to_string(),
            password_hash: String::new(),
            role: user::UserRole::Viewer,
            is_suspended: true,
            suspended_until: None,
            suspension_reason: Some("spam".to_string()),
            created_at: Utc::now(),
        }
    }

    fn temporarily_suspended_user(id: Uuid) -> user::Model {
        user::Model {
            id,
            email: "temp@example.com".to_string(),
            password_hash: String::new(),
            role: user::UserRole::Viewer,
            is_suspended: true,
            suspended_until: Some(Utc::now() + Duration::hours(1)),
            suspension_reason: None,
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn cache_hit_active_allows() {
        let cache = InMemoryCache::new();
        let uid = Uuid::new_v4();
        let key = mediamtx::keys::user_state(&uid);
        cache.set(&key, "active", Some(300)).await.unwrap();

        let db = MockDatabase::new(DbBackend::Postgres).into_connection();
        let result = load_user_access_state(&cache, &db, uid).await.unwrap();
        assert_eq!(result, AccessState::Active);
    }

    #[tokio::test]
    async fn cache_hit_suspended_denies() {
        let cache = InMemoryCache::new();
        let uid = Uuid::new_v4();
        let key = mediamtx::keys::user_state(&uid);
        cache.set(&key, "suspended", None).await.unwrap();

        let db = MockDatabase::new(DbBackend::Postgres).into_connection();
        let result = load_user_access_state(&cache, &db, uid).await.unwrap();
        assert_eq!(result, AccessState::Suspended);
    }

    #[tokio::test]
    async fn cache_miss_active_user_allows_and_rehydrates() {
        let cache = InMemoryCache::new();
        let uid = Uuid::new_v4();
        let user = active_user(uid);

        let db = MockDatabase::new(DbBackend::Postgres)
            .append_query_results([vec![user]])
            .into_connection();

        let result = load_user_access_state(&cache, &db, uid).await.unwrap();
        assert_eq!(result, AccessState::Active);

        // Verify Redis was rehydrated with "active"
        let key = mediamtx::keys::user_state(&uid);
        let cached = cache.get(&key).await.unwrap();
        assert_eq!(cached.as_deref(), Some("active"));
    }

    #[tokio::test]
    async fn cache_miss_suspended_user_denies_and_rehydrates() {
        let cache = InMemoryCache::new();
        let uid = Uuid::new_v4();
        let user = permanently_suspended_user(uid);

        let db = MockDatabase::new(DbBackend::Postgres)
            .append_query_results([vec![user]])
            .into_connection();

        let result = load_user_access_state(&cache, &db, uid).await.unwrap();
        assert_eq!(result, AccessState::Suspended);

        // Verify Redis was rehydrated with "suspended"
        let key = mediamtx::keys::user_state(&uid);
        let cached = cache.get(&key).await.unwrap();
        assert_eq!(cached.as_deref(), Some("suspended"));
    }

    #[tokio::test]
    async fn cache_miss_temp_suspended_denies_with_ttl() {
        let cache = InMemoryCache::new();
        let uid = Uuid::new_v4();
        let user = temporarily_suspended_user(uid);

        let db = MockDatabase::new(DbBackend::Postgres)
            .append_query_results([vec![user]])
            .into_connection();

        let result = load_user_access_state(&cache, &db, uid).await.unwrap();
        assert_eq!(result, AccessState::Suspended);

        // Verify Redis was rehydrated with "suspended" and has a TTL
        let key = mediamtx::keys::user_state(&uid);
        let cached = cache.get(&key).await.unwrap();
        assert_eq!(cached.as_deref(), Some("suspended"));
        let ttl = cache.ttl(&key).await.unwrap();
        assert!(ttl > 0, "suspended key should have TTL, got {ttl}");
    }

    #[tokio::test]
    async fn user_not_found_denies() {
        let cache = InMemoryCache::new();
        let uid = Uuid::new_v4();

        let db = MockDatabase::new(DbBackend::Postgres)
            .append_query_results::<user::Model, _, _>([vec![]])
            .into_connection();

        let result = load_user_access_state(&cache, &db, uid).await.unwrap();
        assert_eq!(result, AccessState::Suspended);
    }

    #[tokio::test]
    async fn unknown_cache_value_falls_back_to_db() {
        let cache = InMemoryCache::new();
        let uid = Uuid::new_v4();
        let key = mediamtx::keys::user_state(&uid);
        // Pollute cache with unknown value
        cache.set(&key, "corrupted", None).await.unwrap();

        let user = active_user(uid);
        let db = MockDatabase::new(DbBackend::Postgres)
            .append_query_results([vec![user]])
            .into_connection();

        let result = load_user_access_state(&cache, &db, uid).await.unwrap();
        // Should fall back to DB and find active user
        assert_eq!(result, AccessState::Active);

        // Cache should be corrected
        let cached = cache.get(&key).await.unwrap();
        assert_eq!(cached.as_deref(), Some("active"));
    }
}
