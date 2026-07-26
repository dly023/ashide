ALTER TABLE windows RENAME COLUMN restored_workspace_sessions_json TO workspace_sessions_json;
ALTER TABLE terminal_panes DROP COLUMN cli_agent_session_id;
ALTER TABLE terminal_panes DROP COLUMN cli_agent_origin;
ALTER TABLE terminal_panes DROP COLUMN cli_command;
ALTER TABLE terminal_panes DROP COLUMN cli_agent;
