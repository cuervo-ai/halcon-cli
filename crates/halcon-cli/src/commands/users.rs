//! `halcon users` — manage user accounts and roles in the Halcon platform.
//!
//! Subcommands:
//!   add    — provision a new user and assign a role
//!   list   — display all provisioned users and their roles
//!   revoke — remove a user's access
//!
//! DECISION: user records are persisted to `~/.halcon/users.toml` (single-node
//! deployment) rather than a database. This avoids requiring a running server
//! to manage users and keeps the operator workflow simple: edit the file or
//! use these CLI commands.
//!
//! For multi-node / enterprise deployments the expectation is that an identity
//! provider (LDAP, SAML, OIDC) is used and halcon verifies JWT role claims at
//! request time. The local TOML file is the bootstrap mechanism.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use halcon_auth::Role;
use serde::{Deserialize, Serialize};

const USERS_FILENAME: &str = "users.toml";

/// A provisioned user record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRecord {
    pub email: String,
    pub role: String,
    /// ISO-8601 timestamp when the user was added.
    pub added_at: String,
    /// Whether this user account is currently active.
    pub active: bool,
}

/// The users.toml manifest — a map from email → UserRecord.
#[derive(Debug, Default, Serialize, Deserialize)]
struct UsersManifest {
    #[serde(default)]
    users: HashMap<String, UserRecord>,
}

/// Resolve the path to the users manifest file.
///
/// Prefers `HALCON_USERS_FILE` env var, then falls back to `~/.halcon/users.toml`.
fn users_file() -> PathBuf {
    if let Ok(p) = std::env::var("HALCON_USERS_FILE") {
        return PathBuf::from(p);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".halcon")
        .join(USERS_FILENAME)
}

fn load_manifest(path: &PathBuf) -> Result<UsersManifest> {
    if !path.exists() {
        return Ok(UsersManifest::default());
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    toml::from_str(&content)
        .with_context(|| format!("Failed to parse {}", path.display()))
}

fn save_manifest(path: &PathBuf, manifest: &UsersManifest) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    }
    let content = toml::to_string_pretty(manifest)
        .context("Failed to serialize users manifest")?;
    std::fs::write(path, content)
        .with_context(|| format!("Failed to write {}", path.display()))
}

/// `halcon users add --email <email> --role <role>`
///
/// Provisions a new user with the given email and role.
/// Valid roles: Admin, Developer, ReadOnly, AuditViewer.
pub fn add(email: &str, role_str: &str) -> Result<()> {
    add_with_path(email, role_str, &users_file())
}

fn add_with_path(email: &str, role_str: &str, path: &PathBuf) -> Result<()> {
    let role = Role::from_str(role_str)
        .ok_or_else(|| anyhow::anyhow!(
            "Unknown role '{role_str}'. Valid roles: Admin, Developer, ReadOnly, AuditViewer"
        ))?;

    let mut manifest = load_manifest(path)?;

    if manifest.users.contains_key(email) {
        anyhow::bail!("User '{email}' already exists. Use `halcon users revoke` first if you want to reassign.");
    }

    manifest.users.insert(
        email.to_string(),
        UserRecord {
            email: email.to_string(),
            role: role.to_string(),
            added_at: chrono::Utc::now().to_rfc3339(),
            active: true,
        },
    );

    save_manifest(path, &manifest)?;
    println!("User '{email}' added with role '{role}'.");
    Ok(())
}

/// `halcon users list`
///
/// Lists all provisioned users with their role and status.
pub fn list() -> Result<()> {
    list_with_path(&users_file())
}

fn list_with_path(path: &PathBuf) -> Result<()> {
    let manifest = load_manifest(path)?;

    if manifest.users.is_empty() {
        println!("No users provisioned.");
        println!("Add a user: halcon users add --email user@org.com --role Developer");
        return Ok(());
    }

    println!("{:<35} {:<15} {:<8} {}", "Email", "Role", "Active", "Added At");
    println!("{}", "─".repeat(90));

    let mut users: Vec<&UserRecord> = manifest.users.values().collect();
    users.sort_by(|a, b| a.email.cmp(&b.email));

    for user in users {
        let active = if user.active { "yes" } else { "no" };
        println!("{:<35} {:<15} {:<8} {}", user.email, user.role, active, user.added_at);
    }

    println!("\n{} user(s) total.", manifest.users.len());
    Ok(())
}

/// `halcon users revoke --email <email>`
///
/// Marks a user as inactive (soft delete). The record is retained for audit.
pub fn revoke(email: &str) -> Result<()> {
    revoke_with_path(email, &users_file())
}

fn revoke_with_path(email: &str, path: &PathBuf) -> Result<()> {
    let mut manifest = load_manifest(path)?;

    let user = manifest.users.get_mut(email)
        .ok_or_else(|| anyhow::anyhow!("User '{email}' not found."))?;

    if !user.active {
        println!("User '{email}' is already revoked.");
        return Ok(());
    }

    user.active = false;
    save_manifest(path, &manifest)?;
    println!("User '{email}' access revoked (role retained for audit).");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn tmp_path(tmp: &tempfile::TempDir) -> PathBuf {
        tmp.path().join("users.toml")
    }

    #[test]
    fn add_and_list_user() {
        let tmp = tempdir().unwrap();
        let path = tmp_path(&tmp);

        add_with_path("alice@example.com", "Developer", &path).unwrap();

        let manifest = load_manifest(&path).unwrap();
        let user = manifest.users.get("alice@example.com").unwrap();

        assert_eq!(user.role, "Developer");
        assert!(user.active);
    }

    #[test]
    fn duplicate_user_returns_error() {
        let tmp = tempdir().unwrap();
        let path = tmp_path(&tmp);

        add_with_path("bob@example.com", "ReadOnly", &path).unwrap();
        let result = add_with_path("bob@example.com", "Admin", &path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[test]
    fn invalid_role_returns_error() {
        let tmp = tempdir().unwrap();
        let path = tmp_path(&tmp);

        let result = add_with_path("carol@example.com", "SuperAdmin", &path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown role"));
    }

    #[test]
    fn revoke_user_marks_inactive() {
        let tmp = tempdir().unwrap();
        let path = tmp_path(&tmp);

        add_with_path("dave@example.com", "Admin", &path).unwrap();
        revoke_with_path("dave@example.com", &path).unwrap();

        let manifest = load_manifest(&path).unwrap();
        let user = manifest.users.get("dave@example.com").unwrap();
        assert!(!user.active);
    }

    #[test]
    fn revoke_nonexistent_user_returns_error() {
        let tmp = tempdir().unwrap();
        let path = tmp_path(&tmp);

        let result = revoke_with_path("ghost@example.com", &path);
        assert!(result.is_err());
    }

    #[test]
    fn list_empty_users() {
        let tmp = tempdir().unwrap();
        let path = tmp_path(&tmp);

        // Should not panic with empty manifest.
        list_with_path(&path).unwrap();
    }
}
