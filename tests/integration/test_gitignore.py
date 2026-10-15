"""Read-only Git ignore checks; no files, index entries or services are changed."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]


class GitignoreTests(unittest.TestCase):
    def assert_paths(self, paths, ignored):
        result = subprocess.run(
            ["git", "-C", str(ROOT), "check-ignore", "--no-index", "-z", "--stdin"],
            input=b"\0".join(path.encode() for path in paths) + b"\0",
            capture_output=True, check=False,
        )
        self.assertIn(result.returncode, (0, 1), result.stderr.decode())
        actual = set(result.stdout.decode().split("\0")) - {""}
        for path in paths:
            with self.subTest(path=path):
                self.assertEqual(path in actual, ignored)

    def test_private_configuration_and_generated_artifacts_are_ignored(self):
        self.assert_paths([
            ".env", ".env.production.local", "server/config/server.env", "agent/config/agent.env.backup",
            "deploy/local/fleet.json", "Notes/private.txt", "Password/private.txt", "服务器连接方式.md",
            "AGENTS.md", "CLAUDE.md", ".claude/settings.local.json", "config/targets.json",
            "config/managed-targets.json", "config/.managed-targets-123.next", "config/staging.json",
            "config/machine-test.known_hosts", "config/known_hosts", "config/id_ed25519",
            "config/ca.crt", "config/server.key", "config/key.pem", "config/identity.p12",
            "platform-update-result.json", "platform-update.lock", "state/node.json",
            "agent/state/identity.db", "server/state/session.json", "platform-releases/release/manifest.json",
            "platform-update-backup.test/server.env.before-update", "easymanage-controller-backup.test/0",
            "web-backup.test/index.html", "platform-backup-test/server", "backups/database.sql",
            "backup.dump", "database.dump.sql", "database.sql.gz", "state.sqlite3-journal",
            "identity.db-wal", "identity.db-shm", "cache.sqlite-journal", "data/local.json",
            "target/release/easymanage-server", "agent/target/debug/easymanage-agent", "build/generated.h",
            "web/dist/index.html", "docs/.vitepress/dist/index.html", "docs/.vitepress/.temp/app.js",
            "web/node_modules/package/index.js", "web/tsconfig.app.tsbuildinfo", "logs/server.log",
            ".venv/bin/python", "tests/integration/__pycache__/test.cpython-310.pyc",
            "artifacts/agent.exe", "native/output.pdb", "native/libprobe.so.1", "native/debug.dSYM/info",
            "bin/easymanage-server", "out/generated.txt", "native/a.out", ".coverage", ".coverage.test",
        ], True)

    def test_sources_templates_locks_and_reserved_platforms_remain_visible(self):
        self.assert_paths([
            "Cargo.toml", "Cargo.lock", "agent/src/main.rs", "agent/src/platform/linux/inventory.rs",
            "server/src/platform_update.rs", "deploy/linux/operations/platform-release.py",
            "migrations/0008_machine_management.sql", "web/src/components/PlatformUpdateDialog.tsx",
            "web/pnpm-lock.yaml", "docs/pnpm-lock.yaml", "web/package-lock.json", ".python-version",
            ".env.example", ".env.production.example", "server/config/server.env.example",
            "agent/config/agent.env.example", "config/app.env.production.example",
            "deploy/examples/fleet.example.json", "config/targets.example.json", "config/known_hosts.example",
            "deploy/docker/.gitkeep", "deploy/windows/README.md", "deploy/macos/README.md",
            "tests/integration/test_gitignore.py", "tests/fixtures/data/sample.json",
            "agent/src/platform/windows/build/mod.rs", "tests/fixtures/dist/sample.json",
            ".vscode/extensions.json", ".vscode/settings.json",
        ], False)


if __name__ == "__main__":
    unittest.main()
