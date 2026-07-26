-- Hard-cut removed pre-templatable MCP runtime persistence.
DELETE FROM pane_leaves WHERE kind = 'mcp_server';
DROP TABLE IF EXISTS mcp_server_panes;
DROP TABLE IF EXISTS active_mcp_servers;
DROP TABLE IF EXISTS mcp_environment_variables;
