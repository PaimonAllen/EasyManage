# EasyManage

EasyManage is a bare-metal infrastructure monitoring and operations platform. It combines a Rust HTTPS server, a Rust agent, PostgreSQL, an agent-side SQLite queue, and a React/TypeScript console.

The controller can also monitor its own host. Remote agents report over HTTPS, with persistent SSH transport available for hosts that only allow SSH connections. The controller runs natively on Linux and manages Linux or Windows Agents. Windows uses OpenSSH, SFTP, PowerShell 7 and a native SCM service (Full) or a limited login task (Basic). macOS integration is planned; container deployment is not currently supported.

## Features

- **Host overview:** CPU, memory, GPU utilization and memory, disk I/O, network traffic, and CPU/GPU/SSD temperatures, with real ten-minute history and daily hardware inventories.
- **Storage:** Read-only filesystem capacity, mount relationships, partition topology, model-based SMR/CMR annotations, and on-demand directory space analysis.
- **Host inventories:** Users and sessions, network interfaces and connection names, routes, virtual networks, permissions, security settings, installed software, AI agent tools, and programming environments.
- **Proxy analysis:** Import Clash-compatible YAML configurations, monitor proxy HTTPS availability and supported TCP node-port connectivity every three minutes, inspect seven-day history, and run IP reputation checks manually. These probes run on the controller, independently of agent collection.
- **Platform visibility:** Twenty-four-hour server and agent performance summaries, application network accounting, hourly storage usage, collection settings, and Gitea status.
- **Agent management:** Trusted release candidates, updates, restarts, and emergency restart operations. Machine management supports deployment, soft deletion, monitoring pause/resume, and stopping an agent while retaining its files and history.

Missing or inaccessible observations remain unknown. Cached results retain their actual collection timestamps; refreshing a page does not turn a cached value into a new sample.

## Deployment and Privilege Model

Controller deployment mode and agent privileges are independent:

| Mode | Purpose | Deployment files |
| --- | --- | --- |
| User-mode controller | Run the platform under an ordinary account | `deploy/linux/user/` |
| System-mode controller | Run the platform as a system service | `deploy/linux/system/` |
| Basic agent | Collect information visible to its ordinary user | `deploy/linux/user/` |
| Full agent | Collect privileged information through a root system service | `deploy/linux/system/` |

User mode is a supported deployment option. A user-mode controller can manage Full agents, and existing mixed deployments are supported. Repository layout changes do not automatically migrate services or downgrade agent privileges.

The intranet monitoring interface does not currently require a platform login. Terminal sessions, privilege elevation, account operations, and emergency restarts require authentication against the target host through SSH and, where needed, sudo. Full status is confirmed by a recent heartbeat reporting root / UID 0 on Linux or an actual Windows Administrator token and native SID on Windows. Information unavailable to a Basic agent is reported as unknown.

## Build and Deployment

Use the Rust toolchain specified in [Cargo.toml](Cargo.toml), Node.js 22 or later, and the pnpm version declared in [web/package.json](web/package.json). Linux deployment uses native services rather than containers.

Build the server, agent, console, and documentation:

```bash
./scripts/run-server.sh --build
```

Building does not start services. On a configured controller, the build can also publish platform and agent release candidates. Publishing a candidate does not automatically replace running services or agents.

The public manifest at `deploy/examples/fleet.example.json` contains example addresses and is deliberately rejected by the deployment tool. Store real inventory in the Git-ignored `deploy/local/fleet.json`, or provide a custom manifest with `--inventory`. Preserve existing node IDs and SQLite state when updating an installation. Verify SSH host keys before deployment.

PostgreSQL can run locally or remotely. Configure it with `DATABASE_URL` or the standard `PGHOST`, `PGPORT`, `PGDATABASE`, `PGUSER`, `PGPASSWORD`, and `PGSSLMODE` variables. No particular host address, port, or operating-system account is required. The server applies versioned database migrations automatically.

Configuration templates are available in `server/config/server.env.example` and `agent/config/agent.env.example`. Keep real configuration outside the repository with file permissions set to `0600`.

Deployment instructions:

- [Deployment overview](deploy/README.md)
- [PostgreSQL configuration](deploy/database/README.md)
- [System services](deploy/linux/system/README.md)
- [User-mode services](deploy/linux/user/README.md)
- [Build and operations scripts](scripts/README.md)
- [Independent proxy probe core](deploy/third-party/README.md)

## Repository Layout

| Path | Responsibility |
| --- | --- |
| `server/` | Platform APIs, persistence, monitoring, and authenticated host operations |
| `agent/` | Agent lifecycle, host collection, and the local offline queue |
| `agent/src/platform/` | Collector contract, shared probe utilities, and the native Linux backend |
| `protocol/` | Shared request, response, metric, and collection-policy types |
| `runtime/` | Process and storage accounting, networking, and privilege helpers |
| `web/` | React/TypeScript console |
| `docs/` | VitePress documentation, architecture, and historical design documents |
| `migrations/` | Versioned PostgreSQL migrations; released migrations are immutable |
| `deploy/linux/` | System/user installation, bootstrap, operations, and deployment-mode handover |
| `deploy/orchestration/` | Trusted fleet orchestration, currently implemented for Linux |
| `deploy/examples/` | Public configuration and inventory templates |
| `deploy/windows/`, `deploy/macos/` | Windows Agent operations and native integration requirements for macOS |
| `deploy/docker/` | Reserved directory; container deployment is not supported |
| `scripts/diagnostics/` | Read-only diagnostic tools |
| `tests/integration/`, `web/tests/` | Isolated integration tests and frontend checks |

Git tracks source code, migrations, configuration templates, tests, documentation, lockfiles, and deployment scripts. Credentials, private host manifests, local connection notes, runtime databases, dependencies, and build artifacts are excluded. Complete external projects belong in the ignored `third_party/` or `vendor_projects/` directories.

See the [agent documentation](agent/README.md) for the actual agent entry point, the [architecture documentation](docs/architecture.md) for platform boundaries, and the [test conventions](tests/README.md) for verification requirements. Historical design documents describe earlier proposals and do not guarantee current functionality.

## Collection and Performance

The agent uses two asynchronous worker threads. Lightweight network counters remain available for live monitoring, while slow inventory and permission checks run as bounded background tasks. Pending scans do not block live sampling or spawn duplicate collectors.

Current independent inventory intervals are three minutes for users, five minutes for proxy processes and AI agent tools, one hour for software and running services, and twelve hours for programming tools. These intervals require an updated agent. Each category reports its own collection time and retains it while cached.

GPU probe failures back off for at least sixty seconds. Application storage usage is collected hourly, hardware details daily, and network filesystem capacity hourly. Directory space analysis starts only when requested. SQLite uses a single WAL connection and a bounded offline queue; updates preserve node identity and unacknowledged reports.

Page refresh intervals are separate from host collection and independent website probes. See [platform behavior and collection intervals](docs/platform.md) for details.

## Operational Safety

Treat managed hosts as production systems. Compilation and static checks may run in the workspace. Runtime, failure-recovery, privilege, and load tests must run in an isolated environment with private resources rather than against production services.

Deployment, database migration, key rotation, and forced restarts change real services. Confirm the target and an appropriate maintenance window before performing those operations. Existing production services retain their deployment mode, and privileged agents retain Full mode unless an explicit change is requested.

Windows requirements, deployment modes and validation boundaries: [Windows Agent](deploy/windows/README.md).
