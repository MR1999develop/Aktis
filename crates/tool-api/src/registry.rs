//! The set of tools available in this build.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use core_model::ToolId;

use crate::Tool;
use crate::descriptor::DescriptorError;

/// Why a tool could not be registered.
#[derive(Debug, Clone, PartialEq)]
pub enum RegistryError {
    /// Another tool already uses this id.
    DuplicateTool(ToolId),
    /// The tool's descriptor is inconsistent.
    InvalidDescriptor(ToolId, DescriptorError),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateTool(id) => write!(f, "tool '{id}' is registered twice"),
            Self::InvalidDescriptor(id, e) => write!(f, "tool '{id}' has an invalid descriptor: {e}"),
        }
    }
}

impl std::error::Error for RegistryError {}

/// Tools by id. Built once at start-up by the composition root, then shared read-only.
#[derive(Debug, Default, Clone)]
pub struct ToolRegistry {
    tools: BTreeMap<ToolId, Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a tool after checking its descriptor.
    ///
    /// # Errors
    /// [`RegistryError`] when the id is taken or the descriptor is invalid.
    pub fn register(&mut self, tool: Arc<dyn Tool>) -> Result<(), RegistryError> {
        let d = tool.descriptor();
        d.validate().map_err(|e| RegistryError::InvalidDescriptor(d.id.clone(), e))?;
        if self.tools.contains_key(&d.id) {
            return Err(RegistryError::DuplicateTool(d.id.clone()));
        }
        self.tools.insert(d.id.clone(), tool);
        Ok(())
    }

    /// The tool with this id, if this build has it.
    #[must_use]
    pub fn get(&self, id: &ToolId) -> Option<&Arc<dyn Tool>> {
        self.tools.get(id)
    }

    /// All tools in id order.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn Tool>> {
        self.tools.values()
    }
}
