ALTER TABLE terminal_panes ADD COLUMN cli_agent TEXT;
ALTER TABLE terminal_panes ADD COLUMN cli_command TEXT;
ALTER TABLE terminal_panes ADD COLUMN cli_agent_origin TEXT;
ALTER TABLE terminal_panes ADD COLUMN cli_agent_session_id TEXT;
ALTER TABLE terminal_panes ADD COLUMN source_identity_keys TEXT;

UPDATE terminal_panes
SET cli_agent = json_extract(
        (SELECT session_binding_json FROM pane_container_identities WHERE pane_node_id = terminal_panes.id),
        '$.agent'
    ),
    cli_command = json_extract(
        (SELECT session_binding_json FROM pane_container_identities WHERE pane_node_id = terminal_panes.id),
        '$.command'
    ),
    cli_agent_origin = json_quote(json_extract(
        (SELECT session_binding_json FROM pane_container_identities WHERE pane_node_id = terminal_panes.id),
        '$.origin'
    )),
    cli_agent_session_id = json_extract(
        (SELECT session_binding_json FROM pane_container_identities WHERE pane_node_id = terminal_panes.id),
        '$.session_id'
    ),
    source_identity_keys = json_extract(
        (SELECT session_binding_json FROM pane_container_identities WHERE pane_node_id = terminal_panes.id),
        '$.source_identity_keys'
    )
WHERE EXISTS (
    SELECT 1 FROM pane_container_identities
    WHERE pane_node_id = terminal_panes.id
      AND session_binding_json IS NOT NULL
);

ALTER TABLE pane_container_identities DROP COLUMN session_binding_json;
