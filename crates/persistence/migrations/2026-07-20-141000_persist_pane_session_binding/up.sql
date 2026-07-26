-- Session binding belongs to the semantic pane container, not to the current
-- terminal/runtime-placeholder carrier. Move the existing terminal metadata to
-- the same generic owner as the stable container UUID, then remove the old
-- terminal-specific copy so future carriers cannot silently omit it.
ALTER TABLE pane_container_identities ADD COLUMN session_binding_json TEXT;

UPDATE pane_container_identities
SET session_binding_json = (
    SELECT json_object(
        'agent', terminal_panes.cli_agent,
        'command', terminal_panes.cli_command,
        'origin', CASE
            WHEN terminal_panes.cli_agent_origin IS NULL THEN NULL
            ELSE json_extract(terminal_panes.cli_agent_origin, '$')
        END,
        'session_id', terminal_panes.cli_agent_session_id,
        'cwd', terminal_panes.cwd,
        'source_identity_keys', CASE
            WHEN terminal_panes.source_identity_keys IS NULL THEN json('[]')
            ELSE json(terminal_panes.source_identity_keys)
        END
    )
    FROM terminal_panes
    WHERE terminal_panes.id = pane_container_identities.pane_node_id
)
WHERE EXISTS (
    SELECT 1
    FROM terminal_panes
    WHERE terminal_panes.id = pane_container_identities.pane_node_id
      AND (
          terminal_panes.cli_agent IS NOT NULL
          OR terminal_panes.cli_command IS NOT NULL
          OR terminal_panes.cli_agent_origin IS NOT NULL
          OR terminal_panes.cli_agent_session_id IS NOT NULL
      )
);

ALTER TABLE terminal_panes DROP COLUMN source_identity_keys;
ALTER TABLE terminal_panes DROP COLUMN cli_agent_session_id;
ALTER TABLE terminal_panes DROP COLUMN cli_agent_origin;
ALTER TABLE terminal_panes DROP COLUMN cli_command;
ALTER TABLE terminal_panes DROP COLUMN cli_agent;
