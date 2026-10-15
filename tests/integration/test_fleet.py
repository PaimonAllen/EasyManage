"""Pure deployment tests. Run in the same isolated environment as isolated_smoke.py."""
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("fleet", Path(__file__).resolve().parents[2] / "deploy/orchestration/fleet.py")
fleet = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(fleet)


class FleetTests(unittest.TestCase):
    def node(self):
        return {"node_id": "00000000-0000-4000-8000-000000000001", "hostname": "fixture",
                "host": "192.0.2.10", "port": 22, "user": "tester", "ssh_target": "fixture"}

    def test_chrono_nanoseconds_on_python_310(self):
        expected = datetime(2026, 9, 21, 9, 34, 25, 229600, tzinfo=timezone.utc)
        self.assertEqual(fleet.parse_timestamp("2026-09-21T09:34:25.229600957Z"), expected)
        self.assertEqual(fleet.parse_timestamp("2026-09-21T09:34:25.229600+00:00"), expected)

    def test_default_inventory_never_uses_the_public_example(self):
        with patch.object(fleet, "ROOT", Path("/tmp/fixture")):
            self.assertEqual(fleet.default_inventory(), Path("/tmp/fixture/deploy/local/fleet.json"))

    def test_prepared_platforms_are_rejected_before_any_host_command(self):
        for platform in ("windows", "macos", "darwin", "", None):
            node = self.node() | {"platform": platform}
            with self.subTest(platform=platform), patch.object(fleet, "run") as run, \
                 patch.object(fleet, "remote") as remote:
                with self.assertRaisesRegex(ValueError, "Linux targets only"):
                    fleet.node_preflight(node)
                with self.assertRaisesRegex(ValueError, "Linux targets only"):
                    fleet.control_agent(node, "restart")
                run.assert_not_called()
                remote.assert_not_called()

    def test_unsupported_target_cannot_publish_a_linux_release(self):
        with patch.object(fleet, "atomic_copy") as copy:
            with self.assertRaisesRegex(ValueError, "Linux targets only"):
                fleet.publish({"nodes": [self.node() | {"platform": "windows"}]}, Path("/tmp/fixture"))
            copy.assert_not_called()

    def test_old_linux_inventory_does_not_require_a_new_platform_field(self):
        fleet.require_linux(self.node())
        fleet.require_linux(self.node() | {"platform": "linux"})

    def test_controller_host_is_guarded_before_reading_inventory(self):
        with patch.object(fleet.sys, "platform", "darwin"), \
             patch.object(fleet.sys, "argv", ["fleet.py", "status"]), \
             patch.object(fleet, "run") as run, patch.object(fleet, "remote") as remote:
            with self.assertRaisesRegex(RuntimeError, "Linux controller"):
                fleet.main()
            run.assert_not_called()
            remote.assert_not_called()

    def test_unsafe_ssh_target_is_rejected_before_any_command(self):
        for target in ("-oProxyCommand=touch", "node;echo unsafe", "$(hostname)"):
            node = self.node() | {"ssh_target": target}
            with patch.object(fleet, "run") as run:
                with self.assertRaises(ValueError):
                    fleet.node_preflight(node)
                run.assert_not_called()

    def test_ssh_alias_cannot_silently_target_another_machine(self):
        result = SimpleNamespace(stdout="hostname 192.0.2.99\nport 22\n", returncode=0)
        with patch.object(fleet, "run", return_value=result), patch.object(fleet, "remote") as remote:
            with self.assertRaisesRegex(ValueError, "host/port disagree"):
                fleet.node_preflight(self.node())
            remote.assert_not_called()

    def test_private_bundle_manifest_covers_every_payload(self):
        with tempfile.TemporaryDirectory(prefix="easymanage-fleet-test.", dir="/tmp") as temporary:
            root = Path(temporary)
            source, bundle = root / "source", root / "bundle"
            bundle.mkdir(mode=0o700)
            for name in ("target/release/easymanage-agent", "deploy/linux/user/install-user-agent.sh",
                         "deploy/linux/operations/install-full-candidate.sh",
                         "deploy/linux/migrations/migrate-agent-to-user.sh", "deploy/linux/migrations/handover.sh",
                         "deploy/linux/user/easymanage-agent.user.service"):
                path = source / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("isolated fixture, never executed")
            ca = root / "ca.crt"
            ca.write_text("isolated CA fixture")
            node = self.node() | {"agent_home": "/home/tester/.easymanage"}
            with patch.object(fleet, "ROOT", source):
                fleet.create_bundle(bundle, node, "a" * 64, "https://192.0.2.10:8443", ca)
            expected = {p.name for p in bundle.iterdir() if p.name != "SHA256SUMS"}
            actual = set()
            for line in (bundle / "SHA256SUMS").read_text().splitlines():
                digest, name = line.split("  ")
                self.assertEqual(digest, hashlib.sha256((bundle / name).read_bytes()).hexdigest())
                actual.add(name)
            self.assertEqual(actual, expected)
            self.assertEqual((bundle / "agent.env").stat().st_mode & 0o777, 0o600)
            self.assertEqual(json.loads((bundle / "node.json").read_text())["node_id"], node["node_id"])

    def test_publish_records_pinned_candidate_without_changing_running_agent(self):
        with tempfile.TemporaryDirectory(prefix="easymanage-publish-test.", dir="/tmp") as temporary:
            base, source = Path(temporary) / "base", Path(temporary) / "source"
            (base / "config").mkdir(parents=True)
            (base / "config/server.env").write_text("fixture=true\n")
            names = ("target/release/easymanage-agent", "deploy/orchestration/fleet.py",
                     "deploy/linux/user/install-user-agent.sh", "deploy/linux/operations/install-full-candidate.sh",
                     "deploy/linux/migrations/migrate-agent-to-user.sh", "deploy/linux/migrations/handover.sh",
                     "deploy/linux/user/easymanage-agent.user.service")
            for name in names:
                path = source / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("isolated fixture")
            inventory = source / "deploy/examples/fleet.example.json"
            node = self.node()
            inventory.parent.mkdir(parents=True, exist_ok=True)
            inventory.write_text(json.dumps({"nodes": [node]}))
            registry = {"targets": [{"node_id": node["node_id"], "hostname": node["hostname"],
                                     "host": node["host"], "port": node["port"],
                                     "default_user": node["user"], "agent_sha256": "old-running-hash"}]}
            (base / "config/targets.json").write_text(json.dumps(registry))
            with patch.object(fleet, "ROOT", source), patch.object(fleet, "BASE", base):
                fleet.publish({"nodes": [node]}, inventory)
            published = json.loads((base / "config/targets.json").read_text())["targets"][0]
            self.assertEqual(published["agent_sha256"], "old-running-hash")
            self.assertEqual(published["ssh_target"], "fixture")
            self.assertEqual(published["release_sha256"], hashlib.sha256(b"isolated fixture").hexdigest())
            self.assertEqual(json.loads((base / "releases/current/deploy/local/fleet.json").read_text()), {"nodes": [node]})
            self.assertTrue((base / "releases/current/deploy/orchestration/fleet.py").is_file())
            self.assertTrue((base / "releases/current/deploy/linux/user/install-user-agent.sh").is_file())
            self.assertFalse((base / "releases/current/deploy/linux/fleet.py").exists())

    def test_restart_routes_only_to_the_selected_trusted_node(self):
        node = self.node()
        with patch.object(fleet, "trusted_node", return_value=node) as trusted, \
             patch.object(fleet, "control_agent", return_value=True) as control:
            self.assertEqual(fleet.restart_one(node["node_id"]), 0)
            trusted.assert_called_once_with(node["node_id"])
            control.assert_called_once_with(node, "restart")

    def test_controller_update_can_preserve_existing_service_configuration(self):
        with tempfile.TemporaryDirectory(prefix="controller-unit-test.", dir="/tmp") as temporary:
            source, base = Path(temporary) / "source", Path(temporary) / "base"
            base.mkdir()
            for name in ("target/release/easymanage-server", "web/dist/index.html", "web/dist/assets/new.js", "docs/.vitepress/dist/index.html"):
                path = source / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("fixture")
            with patch.object(fleet, "ROOT", source), patch.object(fleet, "BASE", base), \
                 patch.object(fleet, "atomic_copy") as copy, patch.object(fleet, "run") as run:
                fleet.update_controller_files(preserve_unit=True)
                self.assertTrue(copy.called)
                self.assertFalse(any(call.args[1].name.endswith(".service") for call in copy.call_args_list))
                run.assert_called_once_with("systemctl", "--user", "restart", "easymanage-server.service")

    def test_basic_restart_refuses_system_agent_before_user_restart(self):
        node = self.node() | {"ssh_target": "local"}
        with patch.object(fleet, "run", return_value=SimpleNamespace(returncode=3)) as run:
            self.assertFalse(fleet.control_agent(node, "restart"))
            command = run.call_args.args
            self.assertEqual(command[:2], ("bash", "-c"))
            self.assertIn("systemctl is-enabled --quiet easymanage-agent.service", command[2])
            self.assertIn("systemctl --user restart easymanage-agent.service", command[2])
            self.assertLess(command[2].index("systemctl is-enabled"), command[2].index("systemctl --user restart"))


if __name__ == "__main__":
    unittest.main()
