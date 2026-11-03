# Implementation Plan: MCP Server

`mcp.rs` holds the JSON-RPC handling behind a `Host` trait, so policy is testable without a window. The window
runs a loopback TCP listener only while enabled, and input goes through a queue it drains on its own thread.
`linecon mcp` is a thin stdio bridge, since MCP clients expect to launch a process.
