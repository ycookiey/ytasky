compile_error!(
    "mcp feature is currently disabled: the MCP handlers still reference the legacy rusqlite db and will be rewritten against ybasey::Database in phase 6. The former implementation is preserved in git history (commit 53fb563~1 and earlier)."
);
