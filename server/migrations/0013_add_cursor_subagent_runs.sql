-- Subagent lifecycles outlive individual canonical history branches.
CREATE TABLE cursor_subagent_runs (
    conversation_id TEXT NOT NULL REFERENCES conversations(conversation_id) ON DELETE CASCADE,
    parent_tool_call_id TEXT NOT NULL CHECK (length(parent_tool_call_id) > 0),
    state BLOB NOT NULL,
    PRIMARY KEY (conversation_id, parent_tool_call_id)
);
