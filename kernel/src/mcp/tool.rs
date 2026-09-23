//! Model Context Protocol (MCP) Tool Specifications
//!
//! Defines declarative tool schemas, required zero-trust capabilities,
//! and invocation handlers for sandboxed agent automations.

use alloc::string::String;
use alloc::vec::Vec;
use crate::capability::CapabilityType;
use crate::task::TaskId;

/// Errors returned by MCP tool execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpError {
    ToolNotFound,
    CapabilityDenied,
    InvalidArguments,
    ExecutionFailed(String),
}

/// Tool execution handler signature.
pub type McpHandler = fn(caller_task: TaskId, args: &[u8]) -> Result<Vec<u8>, McpError>;

/// Declarative MCP Tool definition.
#[derive(Clone)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub required_capability: CapabilityType,
    pub handler: McpHandler,
}
