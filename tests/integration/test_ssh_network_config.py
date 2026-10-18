"""Fixed network switch tests; run only with an isolated /home tmpfs."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sqlite3
import tempfile
import unittest
import uuid

SCRIPT = Path(__file__).resolve().parents[2] / "deploy/linux/operations/configure-ssh-network.py"


class NetworkConfigTests(unittest.TestCase):
    def setUp(self):
        mounts = Path("/proc/self/mountinfo").read_text().splitlines()
        if not any(line.split()[4] == "/home" and line.split(" - ")[1].startswith("tmpfs ") for line in mounts):
            self.skipTest("Requires a private /home tmpfs; never write production homes")
        self.temporary = tempfile.TemporaryDirectory(dir="/home", prefix="network-fixture-")
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name) / ".easymanage"
        (self.base / "config").mkdir(parents=True, mode=0o700)
        (self.base / "state").mkdir()
        (self.base / "state/state.db").write_bytes(b"unchanged fixture identity bytes")
        self.node = str(uuid.uuid4())
        self.ca = self.base / "config/ca.crt"
        self.ca.write_bytes(b"public fixture CA")
        self.path = self.base / "config/agent.env"
        self.original = (f"EASYMANAGE_NODE_ID={self.node}\nEASYMANAGE_SERVER_URL=https://old.example:8443\n"
                         f"EASYMANAGE_ENROLLMENT_TOKEN={'a'*64}\nEXTRA_SETTING=preserved\n").encode()
        self.path.write_bytes(self.original)
        self.path.chmod(0o600)

    def run_switch(self, *, node=None, url="https://127.0.0.1:18443", ca=None, token=None):
        return subprocess.run(["/usr/bin/python3", "-B", str(SCRIPT), str(self.base), node or self.node,
            url, ca or hashlib.sha256(self.ca.read_bytes()).hexdigest(), "basic"],
            input=json.dumps({"enrollment_token": token}).encode(), capture_output=True, timeout=5)

    def test_switch_backs_up_only_env_and_preserves_identity_and_other_settings(self):
        result = self.run_switch(token="b"*64)
        self.assertEqual(result.returncode, 0)
        values = dict(line.split("=",1) for line in self.path.read_text().splitlines())
        self.assertEqual(values["EASYMANAGE_NODE_ID"], self.node)
        self.assertEqual(values["EASYMANAGE_SERVER_URL"], "https://127.0.0.1:18443")
        self.assertEqual(values["EASYMANAGE_ENROLLMENT_TOKEN"], "b"*64)
        self.assertEqual(values["EXTRA_SETTING"], "preserved")
        self.assertEqual((self.base / "state/state.db").read_bytes(), b"unchanged fixture identity bytes")
        backups = list(self.path.parent.glob("agent.env.before-ssh-*"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_bytes(), self.original)
        self.assertEqual(self.path.stat().st_mode & 0o777, 0o600)
        self.assertEqual(backups[0].stat().st_mode & 0o777, 0o600)
        self.assertNotIn(b"b"*64, result.stdout + result.stderr)

    def test_rejects_identity_ca_nonloopback_and_token_injection_before_writes(self):
        for options in ({"node":str(uuid.uuid4())}, {"ca":"0"*64}, {"url":"https://10.0.0.1:18443"},
                        {"url":"https://127.0.0.1:65536"}, {"token":"secret-fixture\nINJECT=bad"}):
            result=self.run_switch(**options)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(self.path.read_bytes(), self.original)
            self.assertFalse(list(self.path.parent.glob("agent.env.before-ssh-*")))
            self.assertNotIn(b"secret-fixture",result.stdout+result.stderr)

    def test_no_renewal_preserves_existing_token_and_symlinks_are_rejected(self):
        self.assertEqual(self.run_switch().returncode, 0)
        self.assertIn("EASYMANAGE_ENROLLMENT_TOKEN="+"a"*64, self.path.read_text())
        original = self.path.with_name("original.env")
        self.path.rename(original)
        self.path.symlink_to(original)
        self.assertNotEqual(self.run_switch().returncode, 0)

    def test_quoted_exported_uuid_and_comments_preserve_other_settings(self):
        original = (f'# private config\nexport EASYMANAGE_NODE_ID = "{self.node}"\n'
                    'EASYMANAGE_SERVER_URL="https://old.example:8443"\n'
                    'EXTRA_SETTING="keep spaces and # literal"\n')
        self.path.write_text(original)
        self.assertEqual(self.run_switch().returncode,0)
        updated=self.path.read_text()
        self.assertIn('EXTRA_SETTING="keep spaces and # literal"',updated)
        self.assertIn('# private config',updated)
        self.assertIn('EASYMANAGE_NODE_ID='+self.node,updated)

    def test_legacy_uuid_can_be_confirmed_from_read_only_sqlite(self):
        self.path.write_text('EASYMANAGE_SERVER_URL=https://old.example:8443\nEXTRA_SETTING=preserved\n')
        state=self.base/'state/state.db'
        state.unlink()
        with sqlite3.connect(state) as db:
            db.execute('CREATE TABLE metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL)')
            db.execute('INSERT INTO metadata VALUES(?,?)',('node_id',self.node))
            db.execute('INSERT INTO metadata VALUES(?,?)',('agent_token','synthetic-agent-token'))
        before=state.read_bytes()
        self.assertEqual(self.run_switch().returncode,0)
        self.assertEqual(state.read_bytes(),before)
        self.assertIn('EASYMANAGE_NODE_ID='+self.node,self.path.read_text())

    def test_specific_sanitized_failure_codes_do_not_create_or_reset_installation(self):
        self.assertEqual(self.run_switch(ca='0'*64).returncode,22)
        self.assertEqual(self.run_switch(node=str(uuid.uuid4())).returncode,23)
        self.path.chmod(0o644)
        self.assertEqual(self.run_switch().returncode,21)
        self.path.chmod(0o600)
        self.path.unlink()
        result=self.run_switch()
        self.assertEqual(result.returncode,20)
        self.assertFalse(self.path.exists())
        self.assertEqual((self.base/'state/state.db').read_bytes(),b'unchanged fixture identity bytes')
