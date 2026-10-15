"""Filesystem-only platform candidate tests: all systemd/readiness calls mocked."""
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("platform_release", Path(__file__).resolve().parents[2] / "deploy/linux/operations/platform-release.py")
release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release)


class PlatformReleaseTests(unittest.TestCase):
    def setUp(self):
        self.old_umask = os.umask(0o077)
        self.temporary = tempfile.TemporaryDirectory(prefix="platform-release-test.", dir="/tmp")
        self.root = Path(self.temporary.name)
        self.base = self.root / "installed"
        self.base.mkdir(mode=0o700)
        self.source = self.root / "source"
        for name, text in [("target/release/easymanage-server", "new-server"), ("web/dist/index.html", "new-console"),
                           ("web/dist/assets/new.js", "new-js"), ("docs/.vitepress/dist/index.html", "new-guide")]:
            path = self.source / name
            path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            path.write_text(text)
            path.chmod(0o600)
        for name, text in [("bin/easymanage-server", "old-server"), ("web/index.html", "old-console"),
                           ("web/guide/index.html", "old-guide"), ("config/server.env", "private-config"),
                           ("agent-state", "preserve-full-agent")]:
            path = self.base / name
            path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            path.write_text(text)
            path.chmod(0o700 if name.startswith("bin/") else 0o600)
        self.base_patch = patch.object(release, "BASE", self.base)
        self.base_patch.start()

    def tearDown(self):
        self.base_patch.stop()
        self.temporary.cleanup()
        os.umask(self.old_umask)

    def publish(self):
        release.publish(self.source)
        return release.candidate()[2]

    def test_publish_does_not_install_or_restart(self):
        with patch.object(release, "service") as service:
            sha = self.publish()
            status = release.status()
            self.assertTrue(status["update_available"])
            self.assertEqual(status["candidate_sha256"], sha)
            self.assertEqual((self.base / "bin/easymanage-server").read_text(), "old-server")
            service.assert_not_called()

    def test_verified_install_platform_only_and_current_detection(self):
        sha = self.publish()
        with patch.object(release, "service") as service, patch.object(release, "ready") as ready:
            release.install(sha, "https://127.0.0.1:8443")
            self.assertEqual((self.base / "bin/easymanage-server").read_text(), "new-server")
            self.assertEqual((self.base / "web/index.html").read_text(), "new-console")
            self.assertEqual((self.base / "web/guide/index.html").read_text(), "new-guide")
            self.assertEqual((self.base / "config/server.env").read_text(), "private-config")
            self.assertEqual((self.base / "agent-state").read_text(), "preserve-full-agent")
            self.assertFalse(release.status()["update_available"])
            self.assertEqual(release.status()["last_result"]["status"], "success")
            self.assertTrue(all("easymanage-server.service" in call.args for call in service.call_args_list))
            ready.assert_called_once()

    def test_readiness_failure_rolls_back_all_existing_files(self):
        sha = self.publish()
        with patch.object(release, "service"), patch.object(release, "ready", side_effect=[RuntimeError("fixture"), None]):
            with self.assertRaises(RuntimeError):
                release.install(sha, "https://127.0.0.1:8443")
        for name, expected in [("bin/easymanage-server", "old-server"), ("web/index.html", "old-console"), ("web/guide/index.html", "old-guide")]:
            self.assertEqual((self.base / name).read_text(), expected)
        self.assertEqual(release.status()["last_result"]["status"], "failed")
        self.assertTrue((self.base / "web/assets/new.js").is_file())
        self.assertEqual((self.base / "agent-state").read_text(), "preserve-full-agent")

    def test_tamper_and_changed_candidate_fail_before_any_service_mutation(self):
        sha = self.publish()
        root, _, _ = release.candidate()
        (root / "web/index.html").write_text("tampered")
        with patch.object(release, "service") as service:
            with self.assertRaises(ValueError):
                release.install(sha, "https://127.0.0.1:8443")
            service.assert_not_called()
        self.publish()
        with self.assertRaises(ValueError):
            release.candidate(sha, verify=True)

    def test_candidate_cannot_write_agent_config_or_escape_root(self):
        for name in ("../outside", "/etc/passwd", "web/../config/server.env", "web//x", "config/server.env", "bin/easymanage-agent"):
            with self.subTest(name=name), self.assertRaises(ValueError):
                release.destination(name)

    def test_failed_restart_and_recovery_report_failure_without_losing_backup(self):
        sha = self.publish()
        def failing_service(*args):
            if args[0] in ("restart", "stop"):
                raise RuntimeError("fixture service manager unavailable")
        with patch.object(release, "service", side_effect=failing_service), patch.object(release, "ready"):
            with self.assertRaises(RuntimeError):
                release.install(sha, "https://127.0.0.1:8443")
        self.assertEqual((self.base / "bin/easymanage-server").read_text(), "old-server")
        outcome = release.status()["last_result"]
        self.assertEqual(outcome["status"], "failed")
        self.assertIn("自动恢复未完成", outcome["message"])
        self.assertTrue(list(self.base.glob("platform-backup-*")))

    def test_shared_home_is_allowed_but_private_platform_root_is_still_required(self):
        self.root.chmod(0o2770)
        sha = self.publish()
        release.candidate(sha, verify=True)
        self.assertTrue(release.status()["update_available"])

    def test_world_writable_home_without_sticky_bit_is_still_rejected(self):
        self.root.chmod(0o777)
        with self.assertRaises(ValueError):
            release.publish(self.source)

    def test_unsafe_platform_root_and_symlinks_are_rejected(self):
        self.base.chmod(0o770)
        with self.assertRaises(ValueError):
            release.publish(self.source)
        self.base.chmod(0o700)
        sha = self.publish()
        target = self.base / "web/index.html"
        target.unlink()
        target.symlink_to(self.base / "config/server.env")
        with patch.object(release, "service") as service, self.assertRaises(ValueError):
            release.install(sha, "https://127.0.0.1:8443")
        self.assertFalse(any(call.args[0] in ("stop", "restart") for call in service.call_args_list))
        self.assertEqual((self.base / "config/server.env").read_text(), "private-config")


if __name__ == "__main__":
    unittest.main()
