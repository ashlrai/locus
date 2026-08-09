(.required_servers | index("locus") != null)
and (.required_servers | index("phantom") != null)
and .mcp_command == "locus-mcp"
and ([.doctor.findings[]?.code] | index("credential_migration_incomplete") == null)
