use serde::{Deserialize, Serialize};

/// Roles that can be assigned to users within a workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Admin,
    Operator,
    Viewer,
}

/// Permissions that can be checked against a role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    AgentUp,
    AgentDown,
    ConfigEdit,
    WorkflowRun,
    BudgetModify,
    Read,
}

/// Check whether the given role grants the requested permission.
pub fn check_permission(role: Role, permission: Permission) -> bool {
    match role {
        Role::Owner => true,
        Role::Admin => !matches!(permission, Permission::BudgetModify),
        Role::Operator => matches!(
            permission,
            Permission::AgentUp | Permission::AgentDown | Permission::WorkflowRun | Permission::Read
        ),
        Role::Viewer => matches!(permission, Permission::Read),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_has_all_permissions() {
        for perm in all_permissions() {
            assert!(
                check_permission(Role::Owner, perm),
                "Owner should have {perm:?}"
            );
        }
    }

    #[test]
    fn admin_has_all_except_budget_modify() {
        for perm in all_permissions() {
            let expected = perm != Permission::BudgetModify;
            assert_eq!(
                check_permission(Role::Admin, perm),
                expected,
                "Admin + {perm:?} should be {expected}"
            );
        }
    }

    #[test]
    fn operator_can_run_agents_and_workflows() {
        assert!(check_permission(Role::Operator, Permission::AgentUp));
        assert!(check_permission(Role::Operator, Permission::AgentDown));
        assert!(check_permission(Role::Operator, Permission::WorkflowRun));
        assert!(check_permission(Role::Operator, Permission::Read));
    }

    #[test]
    fn operator_cannot_edit_config_or_budget() {
        assert!(!check_permission(Role::Operator, Permission::ConfigEdit));
        assert!(!check_permission(Role::Operator, Permission::BudgetModify));
    }

    #[test]
    fn viewer_can_only_read() {
        for perm in all_permissions() {
            let expected = perm == Permission::Read;
            assert_eq!(
                check_permission(Role::Viewer, perm),
                expected,
                "Viewer + {perm:?} should be {expected}"
            );
        }
    }

    #[test]
    fn role_serde_roundtrip() {
        let role = Role::Operator;
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, "\"operator\"");
        let back: Role = serde_json::from_str(&json).unwrap();
        assert_eq!(back, role);
    }

    #[test]
    fn permission_serde_roundtrip() {
        let perm = Permission::WorkflowRun;
        let json = serde_json::to_string(&perm).unwrap();
        assert_eq!(json, "\"workflow_run\"");
        let back: Permission = serde_json::from_str(&json).unwrap();
        assert_eq!(back, perm);
    }

    fn all_permissions() -> Vec<Permission> {
        vec![
            Permission::AgentUp,
            Permission::AgentDown,
            Permission::ConfigEdit,
            Permission::WorkflowRun,
            Permission::BudgetModify,
            Permission::Read,
        ]
    }
}
