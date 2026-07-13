ALTER TABLE terminal_panes ADD COLUMN cli_agent TEXT;
ALTER TABLE terminal_panes ADD COLUMN cli_command TEXT;
ALTER TABLE terminal_panes ADD COLUMN cli_agent_origin TEXT;
ALTER TABLE terminal_panes ADD COLUMN cli_agent_session_id TEXT;

-- `workspace_sessions_json` 混合保存了可由 tabs 推导的 live rows 与真正需要
-- 持久化的 virtual restore targets。硬切为单一语义，旧混合数据不迁移。
ALTER TABLE windows RENAME COLUMN workspace_sessions_json TO restored_workspace_sessions_json;
UPDATE windows SET restored_workspace_sessions_json = NULL;
