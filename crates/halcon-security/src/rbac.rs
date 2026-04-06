//! Role-Based Access Control (RBAC) — formal capability authorization model.
//!
//! Provides typed `Role`, `Resource`, `Action`, `Permission`, and `RbacPolicy`
//! abstractions for the Halcon agent system.
//!
//! # Design
//!
//! Three-tier model:
//! - **Role**: named principal group (Admin, Developer, Readonly, Plugin, Custom)
//! - **Permission**: typed (Action, Resource) pair — what a role *can do* on what
//! - **RbacPolicy**: maps Roles → granted Permission sets; evaluates `can(role, perm)`
//!
//! `Action::All` in the granted set supersedes all specific action checks on the
//! same resource, so `grant(Admin, Permission::all(Bash))` covers Execute, Read,
//! Write, and Delete on Bash without having to enumerate each variant.
//!
//! # Example
//!
//! ```rust
//! use halcon_security::rbac::{RbacPolicy, Role, Permission, Resource};
//!
//! let policy = RbacPolicy::default_halcon_policy();
//! assert!(policy.can(&Role::Developer, &Permission::execute(Resource::Bash)));
//! assert!(!policy.can(&Role::Readonly, &Permission::execute(Resource::Bash)));
//! ```

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

/// Principal groups in the Halcon agent system.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Full system access — can execute arbitrary tools and modify system state.
    Admin,
    /// Standard developer access — tools with path restrictions, no plugin/DB mgmt.
    Developer,
    /// Read-only access — no tool execution, no writes.
    Readonly,
    /// Plugin subprocess — restricted to declared capabilities only.
    Plugin,
    /// Custom role with an explicit string identifier.
    Custom(String),
}

impl Role {
    /// Stable string key used as the `HashMap` key in `RbacPolicy`.
    fn key(&self) -> String {
        match self {
            Role::Admin => "admin".to_string(),
            Role::Developer => "developer".to_string(),
            Role::Readonly => "readonly".to_string(),
            Role::Plugin => "plugin".to_string(),
            Role::Custom(name) => format!("custom:{name}"),
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.key())
    }
}

/// Resource classes that permissions apply to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resource {
    /// Shell command execution (bash, sh, etc.).
    Bash,
    /// File system read operations.
    FileRead,
    /// File system write operations.
    FileWrite,
    /// Network access (HTTP requests, WebSocket).
    Network,
    /// Git repository operations.
    Git,
    /// Plugin management (install, uninstall, enable, disable).
    PluginManagement,
    /// Database access (SQLite reads and writes).
    Database,
    /// System information (process list, environment variables).
    SystemInfo,
    /// A named custom resource.
    Custom(String),
}

impl std::fmt::Display for Resource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Resource::Bash => write!(f, "bash"),
            Resource::FileRead => write!(f, "file_read"),
            Resource::FileWrite => write!(f, "file_write"),
            Resource::Network => write!(f, "network"),
            Resource::Git => write!(f, "git"),
            Resource::PluginManagement => write!(f, "plugin_management"),
            Resource::Database => write!(f, "database"),
            Resource::SystemInfo => write!(f, "system_info"),
            Resource::Custom(name) => write!(f, "custom:{name}"),
        }
    }
}

/// Actions that can be performed on a resource.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Read,
    Write,
    Execute,
    Delete,
    /// Wildcard: grants all actions on the associated resource.
    All,
}

/// A typed permission: (Action, Resource) pair.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Permission {
    pub action: Action,
    pub resource: Resource,
}

impl Permission {
    /// Create a permission from an explicit (action, resource) pair.
    pub fn new(action: Action, resource: Resource) -> Self {
        Self { action, resource }
    }

    /// Shorthand: `Action::Read` on `resource`.
    pub fn read(resource: Resource) -> Self {
        Self::new(Action::Read, resource)
    }

    /// Shorthand: `Action::Write` on `resource`.
    pub fn write(resource: Resource) -> Self {
        Self::new(Action::Write, resource)
    }

    /// Shorthand: `Action::Execute` on `resource`.
    pub fn execute(resource: Resource) -> Self {
        Self::new(Action::Execute, resource)
    }

    /// Shorthand: `Action::Delete` on `resource`.
    pub fn delete(resource: Resource) -> Self {
        Self::new(Action::Delete, resource)
    }

    /// Shorthand: `Action::All` on `resource` (supersedes all specific actions).
    pub fn all(resource: Resource) -> Self {
        Self::new(Action::All, resource)
    }
}

impl std::fmt::Display for Permission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}:{}", self.action, self.resource)
    }
}

/// RBAC policy: maps role keys → granted `Permission` sets.
///
/// `can()` performs a two-step check:
/// 1. Exact match: the requested `(action, resource)` is in the grant set.
/// 2. Wildcard match: `Action::All` on the same resource is in the grant set.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RbacPolicy {
    grants: HashMap<String, HashSet<Permission>>,
}

impl RbacPolicy {
    /// Create an empty policy (no permissions granted to any role).
    pub fn new() -> Self {
        Self::default()
    }

    /// Create the default Halcon policy with pre-configured role grants.
    ///
    /// | Role      | Bash | FileRead | FileWrite | Network | Git | PluginMgmt | Database | SystemInfo |
    /// |-----------|------|----------|-----------|---------|-----|------------|----------|------------|
    /// | Admin     | All  | All      | All       | All     | All | All        | All      | All        |
    /// | Developer | Exec | Read     | Write     | Read    | All | —          | —        | Read       |
    /// | Readonly  | —    | Read     | —         | —       | —   | —          | —        | Read       |
    /// | Plugin    | —    | Read     | Write     | Read    | —   | —          | —        | —          |
    pub fn default_halcon_policy() -> Self {
        let mut p = Self::new();

        // Admin: unrestricted access to all resource classes.
        for res in [
            Resource::Bash,
            Resource::FileRead,
            Resource::FileWrite,
            Resource::Network,
            Resource::Git,
            Resource::PluginManagement,
            Resource::Database,
            Resource::SystemInfo,
        ] {
            p.grant(Role::Admin, Permission::all(res));
        }

        // Developer: standard execution with path restrictions (enforced elsewhere).
        p.grant(Role::Developer, Permission::execute(Resource::Bash));
        p.grant(Role::Developer, Permission::all(Resource::FileRead)); // Read + Execute for all file tools
        p.grant(Role::Developer, Permission::write(Resource::FileWrite));
        p.grant(Role::Developer, Permission::read(Resource::Network));
        p.grant(Role::Developer, Permission::all(Resource::Git));
        p.grant(Role::Developer, Permission::read(Resource::SystemInfo));

        // Readonly: observation only.
        p.grant(Role::Readonly, Permission::read(Resource::FileRead));
        p.grant(Role::Readonly, Permission::read(Resource::SystemInfo));

        // Plugin: declared-capability subset (no system access).
        p.grant(Role::Plugin, Permission::read(Resource::FileRead));
        p.grant(Role::Plugin, Permission::write(Resource::FileWrite));
        p.grant(Role::Plugin, Permission::read(Resource::Network));

        p
    }

    /// Grant a permission to a role.
    pub fn grant(&mut self, role: Role, permission: Permission) {
        self.grants
            .entry(role.key())
            .or_default()
            .insert(permission);
    }

    /// Revoke a specific permission from a role.
    pub fn revoke(&mut self, role: &Role, permission: &Permission) {
        if let Some(perms) = self.grants.get_mut(&role.key()) {
            perms.remove(permission);
        }
    }

    /// Check whether `role` holds `requested`.
    ///
    /// Returns `true` if:
    /// - The exact `(action, resource)` was granted, **or**
    /// - `Action::All` on the same resource was granted.
    pub fn can(&self, role: &Role, requested: &Permission) -> bool {
        let Some(perms) = self.grants.get(&role.key()) else {
            return false;
        };
        // Exact match.
        if perms.contains(requested) {
            return true;
        }
        // Wildcard: Action::All on the same resource covers all specific actions.
        perms.contains(&Permission::all(requested.resource.clone()))
    }

    /// List all permissions granted to `role`.
    pub fn permissions_for(&self, role: &Role) -> Vec<&Permission> {
        self.grants
            .get(&role.key())
            .map(|s| s.iter().collect())
            .unwrap_or_default()
    }

    /// Return `true` if `role` has **any** grants in this policy.
    pub fn has_any_grant(&self, role: &Role) -> bool {
        self.grants.get(&role.key()).is_some_and(|s| !s.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default() -> RbacPolicy {
        RbacPolicy::default_halcon_policy()
    }

    // ─── Role display ────────────────────────────────────────────────────────

    #[test]
    fn role_display_standard() {
        assert_eq!(Role::Admin.to_string(), "admin");
        assert_eq!(Role::Developer.to_string(), "developer");
        assert_eq!(Role::Readonly.to_string(), "readonly");
        assert_eq!(Role::Plugin.to_string(), "plugin");
    }

    #[test]
    fn role_display_custom() {
        assert_eq!(
            Role::Custom("analyst".to_string()).to_string(),
            "custom:analyst"
        );
    }

    // ─── Admin: all access ───────────────────────────────────────────────────

    #[test]
    fn admin_can_execute_bash() {
        assert!(default().can(&Role::Admin, &Permission::execute(Resource::Bash)));
    }

    #[test]
    fn admin_can_write_files() {
        assert!(default().can(&Role::Admin, &Permission::write(Resource::FileWrite)));
    }

    #[test]
    fn admin_can_manage_plugins() {
        assert!(default().can(&Role::Admin, &Permission::all(Resource::PluginManagement)));
    }

    #[test]
    fn admin_can_access_database() {
        assert!(default().can(&Role::Admin, &Permission::read(Resource::Database)));
    }

    // ─── Developer ───────────────────────────────────────────────────────────

    #[test]
    fn developer_can_execute_bash() {
        assert!(default().can(&Role::Developer, &Permission::execute(Resource::Bash)));
    }

    #[test]
    fn developer_cannot_manage_plugins() {
        assert!(!default().can(
            &Role::Developer,
            &Permission::execute(Resource::PluginManagement)
        ));
    }

    #[test]
    fn developer_cannot_access_database() {
        assert!(!default().can(&Role::Developer, &Permission::read(Resource::Database)));
    }

    #[test]
    fn developer_can_use_git() {
        assert!(default().can(&Role::Developer, &Permission::execute(Resource::Git)));
        assert!(default().can(&Role::Developer, &Permission::read(Resource::Git)));
    }

    // ─── Readonly ─────────────────────────────────────────────────────────────

    #[test]
    fn readonly_can_read_files() {
        assert!(default().can(&Role::Readonly, &Permission::read(Resource::FileRead)));
    }

    #[test]
    fn readonly_cannot_execute_bash() {
        assert!(!default().can(&Role::Readonly, &Permission::execute(Resource::Bash)));
    }

    #[test]
    fn readonly_cannot_write_files() {
        assert!(!default().can(&Role::Readonly, &Permission::write(Resource::FileWrite)));
    }

    // ─── Plugin ──────────────────────────────────────────────────────────────

    #[test]
    fn plugin_cannot_execute_bash() {
        assert!(!default().can(&Role::Plugin, &Permission::execute(Resource::Bash)));
    }

    #[test]
    fn plugin_can_read_files() {
        assert!(default().can(&Role::Plugin, &Permission::read(Resource::FileRead)));
    }

    #[test]
    fn plugin_can_write_files() {
        assert!(default().can(&Role::Plugin, &Permission::write(Resource::FileWrite)));
    }

    // ─── Custom role ─────────────────────────────────────────────────────────

    #[test]
    fn custom_role_no_grants_by_default() {
        let policy = default();
        let analyst = Role::Custom("analyst".to_string());
        assert!(!policy.can(&analyst, &Permission::read(Resource::FileRead)));
        assert!(!policy.has_any_grant(&analyst));
    }

    #[test]
    fn custom_role_can_be_granted() {
        let mut policy = RbacPolicy::new();
        let auditor = Role::Custom("auditor".to_string());
        policy.grant(auditor.clone(), Permission::read(Resource::Database));
        assert!(policy.can(&auditor, &Permission::read(Resource::Database)));
        assert!(!policy.can(&auditor, &Permission::write(Resource::Database)));
    }

    // ─── Action::All wildcard ─────────────────────────────────────────────────

    #[test]
    fn action_all_covers_specific_actions() {
        let mut policy = RbacPolicy::new();
        policy.grant(Role::Admin, Permission::all(Resource::Bash));

        // Action::All should cover Read, Write, Execute, Delete.
        assert!(policy.can(&Role::Admin, &Permission::read(Resource::Bash)));
        assert!(policy.can(&Role::Admin, &Permission::write(Resource::Bash)));
        assert!(policy.can(&Role::Admin, &Permission::execute(Resource::Bash)));
        assert!(policy.can(&Role::Admin, &Permission::delete(Resource::Bash)));
    }

    #[test]
    fn action_all_does_not_cover_different_resource() {
        let mut policy = RbacPolicy::new();
        policy.grant(Role::Admin, Permission::all(Resource::Bash));
        // All on Bash does NOT grant anything on FileRead.
        assert!(!policy.can(&Role::Admin, &Permission::read(Resource::FileRead)));
    }

    // ─── Grant / Revoke ──────────────────────────────────────────────────────

    #[test]
    fn revoke_removes_permission() {
        let mut policy = RbacPolicy::default_halcon_policy();
        assert!(policy.can(&Role::Developer, &Permission::execute(Resource::Bash)));
        policy.revoke(&Role::Developer, &Permission::execute(Resource::Bash));
        assert!(!policy.can(&Role::Developer, &Permission::execute(Resource::Bash)));
    }

    #[test]
    fn revoke_nonexistent_is_noop() {
        let mut policy = RbacPolicy::new();
        // Should not panic.
        policy.revoke(&Role::Readonly, &Permission::execute(Resource::Bash));
    }

    // ─── Unknown role ─────────────────────────────────────────────────────────

    #[test]
    fn unknown_role_always_denied() {
        let policy = default();
        let unknown = Role::Custom("ghost".to_string());
        assert!(!policy.can(&unknown, &Permission::read(Resource::FileRead)));
    }

    // ─── Serialization ───────────────────────────────────────────────────────

    #[test]
    fn policy_serialization_round_trip() {
        let original = RbacPolicy::default_halcon_policy();
        let json = serde_json::to_string(&original).unwrap();
        let decoded: RbacPolicy = serde_json::from_str(&json).unwrap();

        // Spot-check a few grants survive the round-trip.
        assert!(decoded.can(&Role::Admin, &Permission::execute(Resource::Bash)));
        assert!(decoded.can(&Role::Developer, &Permission::execute(Resource::Bash)));
        assert!(!decoded.can(&Role::Readonly, &Permission::execute(Resource::Bash)));
    }

    #[test]
    fn permissions_for_returns_all_grants() {
        let policy = default();
        let dev_perms = policy.permissions_for(&Role::Developer);
        assert!(!dev_perms.is_empty());
        // Developer should have execute-bash in the list.
        assert!(dev_perms.contains(&&Permission::execute(Resource::Bash)));
    }
}
