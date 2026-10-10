//! `auth.device_codes` — a device authorization (RFC 8628) the built-in
//! provider is holding for the CLI: nobody's row until a person approves
//! its code in the console, so every lane on it is the maintenance lane.

use chrono::{DateTime, Utc};
use telmoni_shared::UserId;
use telmoni_shared::db::tenant_session::{Maintenance, Scoped};
use uuid::Uuid;

use crate::db::AuthLane;

/// The `status` column, in the words its CHECK admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
enum Status {
    Pending,
    Approved,
    Denied,
}

/// Where a device authorization stands.
#[derive(Debug)]
pub enum DeviceState {
    /// Waiting for the person.
    Pending,
    /// Refused: the next poll is told so.
    Denied,
    /// Approved: the next poll is granted.
    Approved {
        /// The person who approved it.
        user_id: UserId,
        /// The session minted for the device.
        sid: String,
    },
}

/// One authorization, as a poll reads it.
#[derive(Debug)]
pub struct Device {
    pub id: Uuid,
    pub state: DeviceState,
    pub expires_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    status: Status,
    approved_by: Option<UserId>,
    sid: Option<String>,
    expires_at: DateTime<Utc>,
}

impl Row {
    fn into_device(self) -> sqlx::Result<Device> {
        let state = match (self.status, self.approved_by, self.sid) {
            (Status::Pending, ..) => DeviceState::Pending,
            (Status::Denied, ..) => DeviceState::Denied,
            (Status::Approved, Some(user_id), Some(sid)) => DeviceState::Approved { user_id, sid },
            // The CHECK pairs an approval with its person and their session,
            // so a row without them is one nothing here wrote: an error to
            // answer, never a device left waiting out its code.
            (Status::Approved, ..) => {
                return Err(sqlx::Error::Decode(
                    "an approved device authorization lacks the person or the session it grants"
                        .into(),
                ));
            }
        };
        Ok(Device {
            id: self.id,
            state,
            expires_at: self.expires_at,
        })
    }
}

/// Open an authorization for a device that just asked.
pub async fn create(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    device_code_hash: &str,
    user_code: &str,
    interval_secs: i32,
    expires_at: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO auth.device_codes
             (id, device_code_hash, user_code, interval_secs, expires_at, shard_key)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::now_v7())
    .bind(device_code_hash)
    .bind(user_code)
    .bind(interval_secs)
    .bind(expires_at)
    .bind(Uuid::new_v4())
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// The authorization a device is polling for, locked for the poll's
/// decision. `None` is a device code this service never minted.
pub async fn find_for_poll(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    device_code_hash: &str,
) -> sqlx::Result<Option<Device>> {
    sqlx::query_as::<_, Row>(
        "SELECT id, status, approved_by, sid, expires_at
           FROM auth.device_codes WHERE device_code_hash = $1
            FOR UPDATE",
    )
    .bind(device_code_hash)
    .fetch_optional(tx.conn())
    .await?
    .map(Row::into_device)
    .transpose()
}

/// Note a poll, for the interval the next one is held to: `true` when it came
/// before the last one's interval was out. ⚠ One statement on the database's
/// clock, the one that stamps `last_polled_at`: measured on the server's,
/// any skew between the two moved the interval.
pub async fn touch(tx: &mut Scoped<'_, Maintenance<AuthLane>>, id: Uuid) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "WITH polled AS (
            SELECT last_polled_at + make_interval(secs => interval_secs) > now() AS too_soon
              FROM auth.device_codes WHERE id = $1
         ), touched AS (
            UPDATE auth.device_codes SET last_polled_at = now() WHERE id = $1
         )
         SELECT COALESCE(too_soon, false) FROM polled",
    )
    .bind(id)
    .fetch_one(tx.conn())
    .await
}

/// The authorization is finished, whichever way: granted, refused or lapsed.
pub async fn delete(tx: &mut Scoped<'_, Maintenance<AuthLane>>, id: Uuid) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM auth.device_codes WHERE id = $1")
        .bind(id)
        .execute(tx.conn())
        .await?;
    Ok(())
}

/// The person approves the device showing `user_code`, and the device gets
/// `sid` as its session. `false` when no device is waiting for that code.
pub async fn approve(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    user_code: &str,
    user_id: &UserId,
    sid: &str,
) -> sqlx::Result<bool> {
    let done = sqlx::query(
        "UPDATE auth.device_codes
            SET status = 'approved', approved_by = $2, sid = $3
          WHERE user_code = $1 AND status = 'pending' AND expires_at > now()",
    )
    .bind(user_code)
    .bind(user_id)
    .bind(sid)
    .execute(tx.conn())
    .await?;
    Ok(done.rows_affected() == 1)
}

/// The person refuses the device showing `user_code`.
pub async fn deny(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    user_code: &str,
) -> sqlx::Result<bool> {
    let done = sqlx::query(
        "UPDATE auth.device_codes SET status = 'denied'
          WHERE user_code = $1 AND status = 'pending' AND expires_at > now()",
    )
    .bind(user_code)
    .execute(tx.conn())
    .await?;
    Ok(done.rows_affected() == 1)
}

/// The retention sweep's share: authorizations nobody finished.
pub async fn delete_expired(tx: &mut Scoped<'_, Maintenance<AuthLane>>) -> sqlx::Result<u64> {
    let done = sqlx::query("DELETE FROM auth.device_codes WHERE expires_at < now()")
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(status: Status, approved_by: Option<UserId>, sid: Option<&str>) -> Row {
        Row {
            id: Uuid::nil(),
            status,
            approved_by,
            sid: sid.map(str::to_owned),
            expires_at: Utc::now(),
        }
    }

    #[test]
    fn an_approval_reads_with_the_person_and_the_session_it_grants() {
        let person = UserId::new();
        let device = row(Status::Approved, Some(person.clone()), Some("ses_a"))
            .into_device()
            .unwrap();
        let DeviceState::Approved { user_id, sid } = device.state else {
            panic!("read as {:?}", device.state);
        };
        assert_eq!(user_id, person);
        assert_eq!(sid, "ses_a");
    }

    /// ⚠ Never read as pending: its poll would wait out the code, and nothing
    /// would say a row is wrong.
    #[test]
    fn an_approval_without_its_person_or_its_session_fails_to_decode() {
        for (approved_by, sid) in [
            (None, Some("ses_a")),
            (Some(UserId::new()), None),
            (None, None),
        ] {
            assert!(matches!(
                row(Status::Approved, approved_by, sid).into_device(),
                Err(sqlx::Error::Decode(_))
            ));
        }
    }

    #[test]
    fn a_waiting_or_a_refused_device_reads_as_one() {
        let pending = row(Status::Pending, None, None).into_device().unwrap();
        assert!(matches!(pending.state, DeviceState::Pending), "{pending:?}");
        let denied = row(Status::Denied, None, None).into_device().unwrap();
        assert!(matches!(denied.state, DeviceState::Denied), "{denied:?}");
    }
}
