// DECISION: The mailbox uses SQLite (already in halcon-storage) rather than
// an in-memory channel because:
// 1. Messages survive process restarts (important for long-running agent teams)
// 2. The audit trail automatically captures all agent-to-agent communication
// 3. SQLite's WAL mode gives us concurrent readers (multiple agents reading)
//    with a single writer, which matches the mailbox access pattern exactly.
// See US-mailbox (PASO 4-A).

use std::sync::Arc;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use halcon_core::error::{HalconError, Result};

use crate::Database;

/// A message in the agent-to-agent mailbox.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MailboxMessage {
    pub id: Uuid,
    pub from_agent: String,
    /// Recipient agent ID, or the special value "broadcast" for team-wide delivery.
    pub to_agent: String,
    pub team_id: Uuid,
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
    /// If set, the message is not delivered after this time.
    pub expires_at: Option<DateTime<Utc>>,
    pub consumed: bool,
}

/// P2P mailbox for agent-to-agent messaging within a team.
///
/// Persists messages in SQLite so they survive process restarts and
/// provide an audit trail. The table uses WAL mode (inherited from the
/// shared Database connection) to allow concurrent reads from multiple
/// simultaneous sub-agents while a single writer inserts.
pub struct Mailbox {
    db: Arc<Database>,
}

impl Mailbox {
    /// Create a new Mailbox backed by the given database.
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Persist a message in the mailbox.
    pub async fn send(&self, msg: MailboxMessage) -> Result<()> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let payload_json = serde_json::to_string(&msg.payload)
                .map_err(|e| HalconError::DatabaseError(format!("serialize payload: {e}")))?;
            db.with_connection(|conn| {
                conn.execute(
                    "INSERT INTO mailbox_messages \
                     (id, from_agent, to_agent, team_id, payload_json, created_at, expires_at, consumed) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)",
                    rusqlite::params![
                        msg.id.to_string(),
                        msg.from_agent,
                        msg.to_agent,
                        msg.team_id.to_string(),
                        payload_json,
                        msg.created_at.to_rfc3339(),
                        msg.expires_at.map(|dt| dt.to_rfc3339()),
                    ],
                )?;
                Ok(())
            })
            .map_err(|e| HalconError::DatabaseError(format!("insert mailbox message: {e}")))?;
            Ok(())
        })
        .await
        .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    /// Retrieve all unconsumed, non-expired messages addressed to `agent_id`
    /// or broadcast to the team.
    pub async fn receive(&self, agent_id: &str, team_id: Uuid) -> Result<Vec<MailboxMessage>> {
        let db = self.db.clone();
        let agent_id = agent_id.to_string();
        let team_id_str = team_id.to_string();
        let now = Utc::now().to_rfc3339();

        tokio::task::spawn_blocking(move || {
            // Collect raw row data first so we don't hold the lock during parsing.
            #[allow(clippy::type_complexity)]
            let rows: Vec<(String, String, String, String, String, String, Option<String>, bool)> =
                db.with_connection(|conn| {
                    let mut stmt = conn.prepare(
                        "SELECT id, from_agent, to_agent, team_id, payload_json, \
                                created_at, expires_at, consumed \
                         FROM mailbox_messages \
                         WHERE team_id = ?1 \
                           AND (to_agent = ?2 OR to_agent = 'broadcast') \
                           AND consumed = 0 \
                           AND (expires_at IS NULL OR expires_at > ?3) \
                         ORDER BY created_at ASC",
                    )?;
                    let rows = stmt.query_map(
                        rusqlite::params![team_id_str, agent_id, now],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, String>(3)?,
                                row.get::<_, String>(4)?,
                                row.get::<_, String>(5)?,
                                row.get::<_, Option<String>>(6)?,
                                row.get::<_, bool>(7)?,
                            ))
                        },
                    )?;
                    rows.collect::<rusqlite::Result<Vec<_>>>()
                })
                .map_err(|e| HalconError::DatabaseError(format!("query receive: {e}")))?;

            // Parse outside the connection lock.
            let mut messages = Vec::with_capacity(rows.len());
            for (id_str, from_agent, to_agent, team_id_str, payload_json,
                 created_at_str, expires_at_str, consumed) in rows
            {
                let id = Uuid::parse_str(&id_str)
                    .map_err(|e| HalconError::DatabaseError(format!("parse id uuid: {e}")))?;
                let team_id = Uuid::parse_str(&team_id_str)
                    .map_err(|e| HalconError::DatabaseError(format!("parse team_id uuid: {e}")))?;
                let payload: serde_json::Value = serde_json::from_str(&payload_json)
                    .map_err(|e| HalconError::DatabaseError(format!("parse payload: {e}")))?;
                let created_at = created_at_str
                    .parse::<DateTime<Utc>>()
                    .map_err(|e| HalconError::DatabaseError(format!("parse created_at: {e}")))?;
                let expires_at = expires_at_str
                    .map(|s| {
                        s.parse::<DateTime<Utc>>()
                            .map_err(|e| HalconError::DatabaseError(format!("parse expires_at: {e}")))
                    })
                    .transpose()?;

                messages.push(MailboxMessage {
                    id,
                    from_agent,
                    to_agent,
                    team_id,
                    payload,
                    created_at,
                    expires_at,
                    consumed,
                });
            }
            Ok(messages)
        })
        .await
        .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    /// Send a broadcast message from `from` to all agents in the team.
    ///
    /// Equivalent to `send()` with `to_agent = "broadcast"`.
    pub async fn broadcast(
        &self,
        from: &str,
        team_id: Uuid,
        payload: serde_json::Value,
    ) -> Result<()> {
        let msg = MailboxMessage {
            id: Uuid::new_v4(),
            from_agent: from.to_string(),
            to_agent: "broadcast".to_string(),
            team_id,
            payload,
            created_at: Utc::now(),
            expires_at: None,
            consumed: false,
        };
        self.send(msg).await
    }

    /// Mark a message as consumed so it is not re-delivered to the same agent.
    pub async fn mark_consumed(&self, msg_id: Uuid) -> Result<()> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            db.with_connection(|conn| {
                conn.execute(
                    "UPDATE mailbox_messages SET consumed = 1 WHERE id = ?1",
                    rusqlite::params![msg_id.to_string()],
                )?;
                Ok(())
            })
            .map_err(|e| HalconError::DatabaseError(format!("mark consumed: {e}")))?;
            Ok(())
        })
        .await
        .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }

    /// Purge all expired messages, returning the number of rows deleted.
    /// Intended to be called periodically by a background scheduler.
    pub async fn purge_expired(&self) -> Result<usize> {
        let db = self.db.clone();
        let now = Utc::now().to_rfc3339();
        tokio::task::spawn_blocking(move || {
            let deleted = db
                .with_connection(|conn| {
                    conn.execute(
                        "DELETE FROM mailbox_messages \
                         WHERE expires_at IS NOT NULL AND expires_at <= ?1",
                        rusqlite::params![now],
                    )
                })
                .map_err(|e| HalconError::DatabaseError(format!("purge expired: {e}")))?;
            Ok(deleted)
        })
        .await
        .map_err(|e| HalconError::Internal(format!("spawn_blocking: {e}")))?
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Helper: open an in-memory database. Migration 037 runs automatically
    /// via `Database::open_in_memory()` → `run_migrations()`.
    fn make_db() -> Arc<Database> {
        Arc::new(Database::open_in_memory().expect("open in-memory db"))
    }

    /// 3 agents in a team: lead broadcasts, both teammates receive the message.
    #[tokio::test]
    async fn test_broadcast_received_by_all_teammates() {
        let db = make_db();
        let mailbox = Mailbox::new(db);
        let team_id = Uuid::new_v4();

        // Lead broadcasts a task assignment.
        mailbox
            .broadcast("agent-lead", team_id, serde_json::json!({"task": "review PR #42"}))
            .await
            .expect("broadcast");

        // Teammate-A receives it.
        let msgs_a = mailbox
            .receive("agent-tm-a", team_id)
            .await
            .expect("receive tm-a");
        assert_eq!(msgs_a.len(), 1, "teammate-A should receive broadcast");
        assert_eq!(msgs_a[0].from_agent, "agent-lead");
        assert_eq!(msgs_a[0].to_agent, "broadcast");

        // Teammate-B also receives it (broadcast is team-wide, not consumed yet).
        let msgs_b = mailbox
            .receive("agent-tm-b", team_id)
            .await
            .expect("receive tm-b");
        assert_eq!(msgs_b.len(), 1, "teammate-B should receive broadcast");
    }

    /// A teammate replies to the lead with a partial result.
    #[tokio::test]
    async fn test_teammate_replies_to_lead() {
        let db = make_db();
        let mailbox = Mailbox::new(db);
        let team_id = Uuid::new_v4();

        // Teammate sends a direct message to the lead.
        let reply = MailboxMessage {
            id: Uuid::new_v4(),
            from_agent: "agent-tm-a".to_string(),
            to_agent: "agent-lead".to_string(),
            team_id,
            payload: serde_json::json!({"status": "partial", "lines_reviewed": 47}),
            created_at: Utc::now(),
            expires_at: None,
            consumed: false,
        };
        mailbox.send(reply).await.expect("send reply");

        // Lead receives it.
        let msgs = mailbox.receive("agent-lead", team_id).await.expect("receive lead");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].from_agent, "agent-tm-a");
        assert_eq!(msgs[0].payload["status"], "partial");
        assert_eq!(msgs[0].payload["lines_reviewed"], 47);

        // An unrelated agent gets nothing.
        let other = mailbox.receive("agent-tm-b", team_id).await.expect("receive other");
        assert!(other.is_empty(), "other agent should not see direct message");
    }

    /// A message with an expired TTL is not delivered.
    #[tokio::test]
    async fn test_expired_message_not_delivered() {
        let db = make_db();
        let mailbox = Mailbox::new(db);
        let team_id = Uuid::new_v4();

        // Insert a message that already expired 1 second ago.
        let past = Utc::now() - chrono::Duration::seconds(1);
        let expired_msg = MailboxMessage {
            id: Uuid::new_v4(),
            from_agent: "agent-lead".to_string(),
            to_agent: "agent-tm-a".to_string(),
            team_id,
            payload: serde_json::json!({"task": "stale"}),
            created_at: past,
            expires_at: Some(past), // already expired
            consumed: false,
        };
        mailbox.send(expired_msg).await.expect("send expired");

        // Should NOT be delivered.
        let msgs = mailbox.receive("agent-tm-a", team_id).await.expect("receive");
        assert!(msgs.is_empty(), "expired message must not be delivered");

        // purge_expired should delete it.
        let deleted = mailbox.purge_expired().await.expect("purge");
        assert_eq!(deleted, 1, "one expired message should be purged");
    }

    /// mark_consumed prevents re-delivery.
    #[tokio::test]
    async fn test_mark_consumed_prevents_redelivery() {
        let db = make_db();
        let mailbox = Mailbox::new(db);
        let team_id = Uuid::new_v4();

        mailbox
            .broadcast("lead", team_id, serde_json::json!({"job": 1}))
            .await
            .expect("broadcast");

        let msgs = mailbox.receive("tm", team_id).await.expect("receive");
        assert_eq!(msgs.len(), 1);

        // Mark consumed.
        mailbox.mark_consumed(msgs[0].id).await.expect("mark consumed");

        // Second receive returns nothing.
        let msgs2 = mailbox.receive("tm", team_id).await.expect("receive 2");
        assert!(msgs2.is_empty(), "consumed message must not be re-delivered");
    }
}
