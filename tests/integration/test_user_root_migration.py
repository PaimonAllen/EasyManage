"""Run under the documented disposable namespaces, never production systemd."""
import importlib.util
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest

spec = importlib.util.spec_from_file_location("user_root", Path(__file__).resolve().parents[2] / "deploy/linux/migrations/migrate-user-root.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class MigrationTests(unittest.TestCase):
    def fixture(self, home):
        old = home / module.OLD
        (old / "config").mkdir(parents=True)
        (old / "state").mkdir()
        (old / "bin").mkdir()
        (old / "state/state.db").write_bytes(b"fixture-persisted-UUID-token-and-queue")
        (old / "bin/easymanage-server").write_bytes(b"old-server")
        (old / "config/server.env").write_text(f"EASYMANAGE_WEB_ROOT={old}/web\nEASYMANAGE_ENROLLMENT_TOKEN=unchanged-secret\nDATABASE_URL=postgresql://fixture:fixture@localhost/db\n")
        (old / "config/targets.json").write_text(json.dumps({"targets":[{"node_id":"original-UUID","agent_home":str(old),"known_hosts":str(old / "config/known_hosts") }]}))
        units = home / ".config/systemd/user"
        units.mkdir(parents=True)
        for unit in module.UNITS:
            (units / unit).write_text("ExecStart=%h/" + module.OLD + "/bin/fixture\n")
        candidate = home / "candidate"
        candidate.write_bytes(b"\x7fELFnew-server")
        return old, candidate

    def manager(self, active):
        states = set(active)
        calls = []
        def control(*args, check=True):
            calls.append(args)
            if args[0] == "is-active": return SimpleNamespace(returncode=0 if args[-1] in states else 3)
            if args[0] == "stop": states.discard(args[-1])
            if args[0] == "start": states.add(args[-1])
            return SimpleNamespace(returncode=0)
        return control, calls

    def test_move_preserves_identity_and_rewrites_only_path_settings(self):
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary)
            old, candidate = self.fixture(home)
            control, calls = self.manager([module.UNITS[0]])
            identity = (old / "state/state.db").read_bytes()
            module.migrate(home, candidate, True, control, lambda root: None)
            new = home / module.NEW
            self.assertFalse(old.exists())
            self.assertEqual((new / "state/state.db").read_bytes(), identity)
            self.assertEqual((new / "bin/easymanage-server").read_bytes(), candidate.read_bytes())
            values = (new / "config/server.env").read_text()
            self.assertIn(str(new / "web"), values)
            self.assertIn("unchanged-secret", values)
            registry = json.loads((new / "config/targets.json").read_text())
            self.assertEqual(registry["targets"][0]["node_id"], "original-UUID")
            self.assertEqual(registry["targets"][0]["agent_home"], str(new))
            self.assertNotIn(("start", module.UNITS[1]), calls)  # Never downgrade Full.
            self.assertEqual(new.stat().st_mode & 0o777, 0o700)

    def test_health_failure_restores_old_files_service_paths_and_binary(self):
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary)
            old, candidate = self.fixture(home)
            before = (old / "config/server.env").read_bytes()
            control, calls = self.manager([module.UNITS[0]])
            def failure(root): raise RuntimeError("fixture-health-failure")
            with self.assertRaisesRegex(RuntimeError, "fixture-health-failure"):
                module.migrate(home, candidate, True, control, failure)
            self.assertFalse((home / module.NEW).exists())
            self.assertEqual((old / "config/server.env").read_bytes(), before)
            self.assertEqual((old / "bin/easymanage-server").read_bytes(), b"old-server")
            self.assertIn(module.OLD, (home / ".config/systemd/user" / module.UNITS[0]).read_text())

    def test_existing_destination_is_never_merged(self):
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary)
            old, candidate = self.fixture(home)
            (home / module.NEW).mkdir()
            with self.assertRaisesRegex(RuntimeError, "Destination already exists"):
                module.migrate(home, candidate)
            self.assertTrue(old.exists())


if __name__ == "__main__": unittest.main()
