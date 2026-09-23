//! Model Context Protocol (MCP) Subsystem
//!
//! Exposes local JSON-RPC / MCP tool schemas, tool execution brokers,
//! and capability verification for AI agent automations.

pub mod registry;
pub mod tool;

pub use registry::{init, McpRegistry, MCP_REGISTRY};
pub use tool::{McpError, McpHandler, McpTool};
