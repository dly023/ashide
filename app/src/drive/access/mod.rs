use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum LocalObjectAccessLevel {
    View,
    Edit,
    Full,
}

impl LocalObjectAccessLevel {
    pub fn label(&self) -> &'static str {
        match self {
            LocalObjectAccessLevel::View => "Can view",
            LocalObjectAccessLevel::Edit => "Can edit",
            LocalObjectAccessLevel::Full => "Full access",
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            LocalObjectAccessLevel::View => "view",
            LocalObjectAccessLevel::Edit => "edit",
            LocalObjectAccessLevel::Full => "access",
        }
    }

    pub fn can_trash(self) -> bool {
        self >= LocalObjectAccessLevel::Edit
    }

    pub fn can_delete(self) -> bool {
        self >= LocalObjectAccessLevel::Full
    }

    pub fn can_move_drive(self) -> bool {
        self >= LocalObjectAccessLevel::Full
    }

    pub fn can_edit_access(self) -> bool {
        self >= LocalObjectAccessLevel::Full
    }

    pub fn to_serializable_value(self) -> &'static str {
        match self {
            LocalObjectAccessLevel::View => "VIEW",
            LocalObjectAccessLevel::Edit => "EDIT",
            LocalObjectAccessLevel::Full => "FULL",
        }
    }
}

impl FromStr for LocalObjectAccessLevel {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "VIEW" => Ok(Self::View),
            "EDIT" => Ok(Self::Edit),
            "FULL" => Ok(Self::Full),
            _ => Err(anyhow::anyhow!("unknown access level {value}")),
        }
    }
}

/// Whether a local object's contents are editable in the current view.
#[derive(Debug, Clone, Copy)]
pub enum ContentEditability {
    ReadOnly,
    Editable,
}

impl ContentEditability {
    pub fn can_edit(self) -> bool {
        matches!(self, ContentEditability::Editable)
    }
}
