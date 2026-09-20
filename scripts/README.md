# Automation scripts

The default launchers compile as the current user, then try to run only the resulting binary
with administrator privileges:

```bash
./scripts/run-server.sh
./scripts/run-agent.sh
```

If elevation is unavailable, the launcher and application both report the privilege problem
and continue in degraded mode. To make missing privileges fatal later:

```bash
EASYMANAGE_ADMIN_POLICY=require ./scripts/run-server.sh
EASYMANAGE_ADMIN_POLICY=require ./scripts/run-agent.sh
```

Scripts that install shared system dependencies belong in `TEMP/Need_Admin/`.
