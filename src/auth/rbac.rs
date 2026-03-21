use serde::{Deserialize, Serialize};

/// Roles that can be assigned to users within a workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Admin,
    Operator,
    Viewer,
}

/// Fine-grained permissions that govern tutti operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    AgentStart,
    AgentStop,
    ConfigEdit,
    WorktreeCreate,
    WorktreeDelete,
    KeyManage,
    RoleAssign,
    AuditRead,
}

impl Role {
    /// Returns the set of permissions granted to this role.
    pub fn permissions(self) -> &'static [Permission] {
        match self {
            Role::Owner => &[
                Permission::AgentStart,
                Permission::AgentStop,
                Permission::ConfigEdit,
                Permission::WorktreeCreate,
                Permission::WorktreeDelete,
                Permission::KeyManage,
                Permission::RoleAssign,
                Permission::AuditRead,
            ],
            Role::Admin => &[
                Permission::AgentStart,
                Permission::AgentStop,
                Permission::ConfigEdit,
                Permission::WorktreeCreate,
                Permission::WorktreeDelete,
                Permission::KeyManage,
                Permission::AuditRead,
            ],
            Role::Operator => &[
                Permission::AgentStart,
                Permission::AgentStop,
                Permission::WorktreeCreate,
            ],
            Role::Viewer => &[Permission::AuditRead],
        }
    }

    /// Check whether this role grants the given permission.
    pub fn has_permission(self, perm: Permission) -> bool {
        self.permissions().contains(&perm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_has_all_permissions() {
        for perm in [
            Permission::AgentStart,
            Permission::AgentStop,
            Permission::ConfigEdit,
            Permission::WorktreeCreate,
            Permission::WorktreeDelete,
            Permission::KeyManage,
            Permission::RoleAssign,
            Permission::AuditRead,
        ] {
            assert!(
                Role::Owner.has_permission(perm),
                "Owner should have {perm:?}"
            );
        }
    }

    #[test]
    fn admin_cannot_assign_roles() {
        assert!(!Role::Admin.has_permission(Permission::RoleAssign));
    }

    #[test]
    fn admin_can_manage_keys() {
        assert!(Role::Admin.has_permission(Permission::KeyManage));
    }

    #[test]
    fn operator_limited_to_agent_and_worktree_create() {
        assert!(Role::Operator.has_permission(Permission::AgentStart));
        assert!(Role::Operator.has_permission(Permission::AgentStop));
        assert!(Role::Operator.has_permission(Permission::WorktreeCreate));
        assert!(!Role::Operator.has_permission(Permission::ConfigEdit));
        assert!(!Role::Operator.has_permission(Permission::WorktreeDelete));
        assert!(!Role::Operator.has_permission(Permission::KeyManage));
        assert!(!Role::Operator.has_permission(Permission::RoleAssign));
        assert!(!Role::Operator.has_permission(Permission::AuditRead));
    }

    #[test]
    fn viewer_can_only_read_audit() {
        assert!(Role::Viewer.has_permission(Permission::AuditRead));
        assert!(!Role::Viewer.has_permission(Permission::AgentStart));
        assert!(!Role::Viewer.has_permission(Permission::AgentStop));
        assert!(!Role::Viewer.has_permission(Permission::ConfigEdit));
        assert!(!Role::Viewer.has_permission(Permission::WorktreeCreate));
        assert!(!Role::Viewer.has_permission(Permission::KeyManage));
        assert!(!Role::Viewer.has_permission(Permission::RoleAssign));
    }

    #[test]
    fn role_serialization_roundtrip() {
        let role = Role::Operator;
        let json = serde_json::to_string(&role).expect("serialize");
        assert_eq!(json, "\"operator\"");
        let parsed: Role = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, role);
    }

    #[test]
    fn permission_serialization_roundtrip() {
        let perm = Permission::WorktreeCreate;
        let json = serde_json::to_string(&perm).expect("serialize");
        assert_eq!(json, "\"worktree_create\"");
        let parsed: Permission = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, perm);
    }
}
